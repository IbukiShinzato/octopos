# octoposにおけるファイルオブジェクト管理と`FileInner`の透過的Restoring

## 1. はじめに

### 1.1 目的

本稿の目的は、Rustで実装されたxv6系OSであるoctoposにおけるファイルオブジェクトの管理方式を整理し、それを踏まえて`FileInner`の回復機構（Restoring）をどのように設計・実装したかを説明することである。具体的には、次の事項を対象とする。

1. 回復機構導入前の`File`、`FileMeta`、`FileInner`、`Inode`の関係と、Rustの所有権・借用・ロックによる管理。
2. `read()`と`write()`の通常処理、および`offset`を含むファイル状態の更新。
3. R-Logを用いた`FileInner`の再構築と差し替え、および両システムコールでの透過的継続。
4. 動作検証で確認できたことと、現段階の制約。

### 1.2 故障モデルと対象範囲

故障対象は、カーネル内の**単一の`FileInner`オブジェクト**とする。試作実装では、`read()`の`readable`確認前、または`write()`の`writeable`確認前に、疑似的な訂正不能メモリエラー（UE）を検出したものとして回復処理を呼び出す。実際にメモリビットを破損させたり、ハードウェアのUE通知を処理したりするものではない。

回復時は、損傷したと想定する旧`FileInner`を直接修復せず、別の領域に新しい`FileInner`を生成して、ファイルテーブル上の参照先を切り替える。以下では特に、`FileType::Inode`に対応する通常ファイルの読み書きに焦点を当てる。`Pipe`や`Device`を含むすべてのケースが検証済みという意味ではない。

## 2. octoposにおけるファイルオブジェクトの管理（変更前）

**本章は回復機構導入前の実装を扱う。** 回復実装に伴って変更した`FileTable.inner`の構造は第4章で説明する。以下に掲載する変更前の構造体は、[導入前の`kernel/src/file.rs`](https://github.com/IbukiShinzato/octopos/blob/0e92fd67885d8517393c659fa3b6da1815d4076e/kernel/src/file.rs)に基づく。

### 2.1 `FD`、`File`、`FileTable`の関係

ユーザ空間の`Fd`はプロセスにおけるファイルディスクリプタであり、その数値は`File.id`と同じとは限らない。カーネルはプロセスが保持する`open_files[fd]`から`File`を取得し、`File.id`を用いてシステム全体の`FILE_TABLE`を参照する。

```text
ユーザ空間の Fd
    │ システムコール
    ▼
プロセスの open_files[fd]
    │ 取得
    ▼
File { id }
    │ インデックスとして利用
    ▼
FILE_TABLE
    ├─ meta[id]  : FileMeta（論理的参照数）
    └─ inner[id] : FileInner（権限・種類・offset）
```

`File`は次のように`id`のみを保持する。`File`自体にファイルの内容や`offset`が格納されているわけではない。

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub id: usize,
}
```

変更前の`FileTable`は、以下のように定義されている。

```rust
pub struct FileTable {
    pub meta: SpinLock<[FileMeta; NFILE]>,
    pub inner: [SleepLock<FileInner>; NFILE],
}
```

- `FileMeta`は`ref_count`を保持し、システム内のファイルオブジェクトに対する論理的な参照を管理する。
- `FileInner`は`readable`、`writeable`、`r#type`、`offset`を保持する。
- `meta`は配列全体を1つの`SpinLock`で保護し、各`inner[id]`は独立した`SleepLock`で保護する。
- 変更前の`inner`は**固定長配列に`SleepLock<FileInner>`を直接格納する構造**であり、`FileInner`を別途ヒープ確保してテーブル要素を差し替える仕組みではない。

`FileInner`および`FileType`の定義は次のとおりである。

```rust
pub struct FileInner {
    pub readable: bool,
    pub writeable: bool,
    pub r#type: FileType,
    pub offset: u32,
}

pub enum FileType {
    None,
    Pipe { pipe: Arc<Pipe> },
    Inode { inode: Inode },
    Device { inode: Inode, major: u16 },
}
```

`readable`と`writeable`はアクセス権限、`offset`は現在のファイル位置、`r#type`は実際の操作対象を表す。`Pipe`、通常ファイル（`Inode`）、デバイスによって後続処理が分岐する。

### 2.2 `Inode`と`INODE_TABLE`

`FileType::Inode`には`Inode`ハンドルが格納される。`Inode`は実ファイルの全メタデータを直接所有するわけではなく、主にinode tableのインデックス（`id`）、デバイス番号（`dev`）、inode番号（`inum`）を保持する。

```rust
pub struct Inode {
    pub id: usize,
    pub dev: u32,
    pub inum: u32,
}

pub struct InodeTable {
    pub meta: SpinLock<[InodeMeta; NINODE]>,
    inner: [SleepLock<InodeInner>; NINODE],
}
```

`InodeMeta`は`dev`、`inum`、`r#ref`を管理し、`InodeInner`はファイルサイズ、ブロックアドレスなどのキャッシュされたinode情報を保持する。`FileInner`のロックと`InodeInner`のロックは別物である。`read()`/`write()`で通常ファイルを処理する場合、`FileInner`のロックを保持した後に`Inode::lock()`によってinode側のロックを取得する。

参照：[`kernel/src/fs.rs`](https://github.com/IbukiShinzato/octopos/blob/e4972dbaf3dc1653d93b7a7bdb5c5b67fdd53f27/kernel/src/fs.rs)

### 2.3 `SpinLock`と`SleepLock`の役割

`SpinLock`は、ロックが取得できるまでCPU上で再試行する排他制御である。octoposの実装では保持中にそのCPUの割り込みを無効化する。比較的短い参照数操作やテーブル管理などに適している。`FileTable.meta`では配列全体を単一のロックで保護するため、異なる`FileMeta`要素の更新であっても同じロックを同時には取得できない。

`SleepLock`は、ロック競合時に待機する**プロセスをスリープさせる**排他制御である。ロックが解放されると待機プロセスが起床し、取得を再試行する。`FileInner`をロックしたまま行うファイルI/Oには、ディスクアクセスなど時間のかかる処理やスリープし得る処理が含まれるため、`SleepLock`が合理的であると考えられる。フィールドの個数それ自体が、`SleepLock`を選択する理由になるわけではない。

また、各`FileInner`に独立したロックを設けることで、異なるファイルオブジェクトを操作する際の不要な排他を減らしている。ただし、これは実装構造上の性質であり、本研究で多CPU実行の性能や安全性を測定したことを意味しない。

参照：[`kernel/src/spinlock.rs`](https://github.com/IbukiShinzato/octopos/blob/e4972dbaf3dc1653d93b7a7bdb5c5b67fdd53f27/kernel/src/spinlock.rs)、[`kernel/src/sleeplock.rs`](https://github.com/IbukiShinzato/octopos/blob/e4972dbaf3dc1653d93b7a7bdb5c5b67fdd53f27/kernel/src/sleeplock.rs)

### 2.4 所有権・借用・`Arc`・RAII

Rustでは値の所有者と参照の有効期間が型および借用規則によって管理される。`File::read()`と`File::write()`では、`FileInner.r#type`を借用しながら`file_inner.offset`にもアクセスするため、`Inode`ハンドルを`clone()`して一時的なローカル値として扱う箇所がある。ここでの`Inode::clone()`はハンドルの値を複製する操作であり、inode tableの論理的参照数を増やさない。

`Arc<T>`はヒープに格納した`T`を複数の所有者が共有するためのスマートポインタである。単にヒープ領域を使うだけなら`Box<T>`でも可能だが、`Box<T>`と異なり、`Arc::clone()`は同一オブジェクトへの共有所有権を増やす。最後の強参照が失われると、管理対象の値と割り当てが解放される。**`Arc`の強参照数と、OS独自の`FileMeta.ref_count`は別の値である。** なお、変更前の`FileTable.inner`自体には`Arc`は使われていない。`Arc<SleepLock<FileInner>>`が導入されるのは第4章の変更後である。

RAII（Resource Acquisition Is Initialization）により、`SpinLockGuard`や`SleepLockGuard`はdrop時にロックを解放する。明示的な`drop(guard)`で先に解放することもできる。ただし、RAIIはロックの取得順序の誤りや再帰取得によるデッドロックを防ぐものではない。さらに、すべてのOS内部資源がdropだけで解放されるわけではなく、inode tableの参照数は`put()`などで明示的に管理している。

### 2.5 参照カウントの種類

| 対象                   | 管理する値           | 主な更新契機                                    | 意味                                         |
| ---------------------- | -------------------- | ----------------------------------------------- | -------------------------------------------- |
| ファイルオブジェクト   | `FileMeta.ref_count` | `File::dup()` / `File::close()`                 | OSが管理する`File.id`への論理的参照数        |
| inode table            | `InodeMeta.r#ref`    | `Inode::get()`、`Inode::dup()` / `Inode::put()` | inode tableエントリへの論理的参照数          |
| 共有ヒープオブジェクト | `Arc`の強参照数      | `Arc::clone()` / drop                           | 同一ヒープオブジェクトのRust側の共有所有者数 |

`File::clone()`は`id`をコピーするだけで、`FileMeta.ref_count`を自動で増やさない。`Inode::clone()`も同様に`InodeMeta.r#ref`を増やさない。したがって、**Rustの値の複製・所有権管理と、OSが管理する論理的な参照数を混同してはならない。** これは回復処理でInodeの参照を引き継ぐ際にも重要になる。

## 3. `read()` / `write()`システムコールの通常処理

本章ではまず故障のない通常経路を扱う。`FileInner`の権限確認、種類による分岐、inodeアクセス、`offset`更新という基本的な流れは、回復実装後にも共通している。

### 3.1 共通する呼び出し経路

```text
ユーザ空間：Fd::read() / Fd::write()
    ↓  Read / Write トレイトの実装（user/src/io.rs）
ユーザ空間：syscall::read() / syscall::write()
    ↓  システムコール
カーネル：sys_read() / sys_write()（kernel/src/sysfile.rs）
    ↓  SyscallArgs::get_file(0) で FD から File を取得
カーネル：File::read() / File::write()（kernel/src/file.rs）
    ↓
FileInner の権限確認 → FileTypeによる分岐
```

最初の引数はFDであり、`sys_read()`/`sys_write()`は`get_file(0)`を利用して対応する`File`を取得する。その後、`File.id`に対応する`FileInner`をロックしてアクセスする。`Pipe`、`Inode`、`Device`で具体的な処理が異なるため、以下は主に通常ファイル（`FileType::Inode`）を対象とする。

### 3.2 通常の`read()`

1. `FileInner`の`SleepLock`を取得する。
2. `readable`が`false`なら`SysError::BadDescriptor`を返す。
3. `FileType::Inode`なら`Inode`ハンドルを`clone()`し、`Inode::lock()`で`InodeInner`のロックを取得する。
4. 現在の`file_inner.offset`を読み込み開始位置として`inode.read()`を呼ぶ。
5. inode側のロックを解放し、読み込み成功時に読み込んだバイト数だけ`offset`を進める。
6. 成功時は`Ok(読み込みバイト数)`、inodeの読み込み失敗時は`SysError::IoError`を返す。権限不備などでは別のエラーになり得る。

`offset`は、プロセスが保持するFDの数値ではなく、対応するファイルオブジェクトの`FileInner`に属する。同じファイルオブジェクトを参照する場合は、その位置を共有する。

### 3.3 通常の`write()`

1. `FileInner`の`SleepLock`を取得する。
2. `writeable`が`false`なら`SysError::BadDescriptor`を返す。
3. `FileType::Inode`なら`Inode`ハンドルを`clone()`し、ファイルシステムのログ容量を超えないよう書き込みをチャンクに分ける。
4. 各チャンクで`Operation::begin()`を行い、inodeのロックを取得して`inode.write()`を呼ぶ。
5. inodeのロックを解放し、ログ操作を終了する。成功したチャンクでは書き込んだバイト数だけ`offset`を進める。
6. 全データを書き込めた場合に`Ok(n)`、途中で失敗した場合などに`SysError::IoError`を返す。

チャンク長の上限は次式で計算している（実装の`MAXOPBLOCKS=10`、`BSIZE=1024`の場合は**3072バイト＝3KiB**）。

```rust
let max = ((MAXOPBLOCKS - 1 - 1 - 2) / 2) * BSIZE;
```

これは**1回のユーザ書き込みを最大3KiBに制限する**という意味ではない。大きな書き込みを、カーネル内部で複数のinode書き込み・ログ操作に分割するという意味である。また、複数チャンクを含むユーザの`write()`全体が単一の不可分なトランザクションになるわけではない。途中失敗時には一部の書き込みが行われていても`IoError`を返し得る。

### 3.4 共通点と相違点

| 観点                       | `read()`                         | `write()`                                    |
| -------------------------- | -------------------------------- | -------------------------------------------- |
| アクセス権限               | `readable`                       | `writeable`                                  |
| inode操作                  | `inode.read()`                   | `inode.write()`                              |
| `FileInner`                | 権限・type・offsetを管理         | 権限・type・offsetを管理                     |
| inodeのロック              | 利用する                         | 利用する                                     |
| 通常ファイルの処理単位     | 基本的に1回のinode読み込み       | 最大3KiB程度のチャンクごと                   |
| ファイルシステムのログ操作 | 通常の読み込み経路では開始しない | チャンクごとに`Operation::begin()`・終了処理 |
| `offset`更新               | 読み込み成功時                   | 各チャンクの書き込み成功時                   |

`Operation`が扱うログは**ディスクブロックの更新用の物理redoログ**であり、第4章の`R-Log`とは異なる。これらを同一のログ機構として説明しない。

参照：[`user/src/io.rs`](https://github.com/IbukiShinzato/octopos/blob/e4972dbaf3dc1653d93b7a7bdb5c5b67fdd53f27/user/src/io.rs)、[`kernel/src/sysfile.rs`](https://github.com/IbukiShinzato/octopos/blob/e4972dbaf3dc1653d93b7a7bdb5c5b67fdd53f27/kernel/src/sysfile.rs)、[`kernel/src/file.rs`](https://github.com/IbukiShinzato/octopos/blob/e4972dbaf3dc1653d93b7a7bdb5c5b67fdd53f27/kernel/src/file.rs)、[`kernel/src/log.rs`](https://github.com/IbukiShinzato/octopos/blob/e4972dbaf3dc1653d93b7a7bdb5c5b67fdd53f27/kernel/src/log.rs)

## 4. `FileInner`の回復方式

### 4.1 回復方針

元の`FileTable.inner`は固定長配列に`SleepLock<FileInner>`を直接保持していたため、故障した要素の参照先を別領域に置き換える構造ではなかった。そこで、`FileInner`をヒープ上で独立に生成し、`FileTable`には参照先を保持する構成へ変更した。

実装した方式では、R-Logから`FileInner`に必要な情報を再構築し、対象スロットの`Arc`を新しいものへ差し替える。故障対象とする1つの`FileInner`のみを交換する設計であり、`FILE_TABLE`やinode table全体を再生成するものではない。

### 4.2 変更後の`FileTable`

```rust
pub struct FileTable {
    pub meta: SpinLock<[FileMeta; NFILE]>,
    pub inner: SpinLock<[Option<Arc<SleepLock<FileInner>>>; NFILE]>,
}
```

```text
FILE_TABLE
 ├─ meta: SpinLock<[FileMeta; NFILE]>
 └─ inner: SpinLock<[Option<Arc<SleepLock<FileInner>>>; NFILE]>
               │ 配列要素が参照を保持
               └── Arc ──► SleepLock<FileInner>（ヒープ上）
                                        │
                                  readable / writeable
                                  r#type / offset
```

各`FileInner`のフィールドは個別の`SleepLock`で保護し、`FILE_TABLE.inner`を保護する新しい`SpinLock`は、配列内の`Arc`の取得・格納・差し替えのために用いる。`get_inner(id)`はテーブルのロックを取得して`Arc::clone()`で参照を取り出し、ロックを解放する。これによって、テーブルをロックし続けずに個別の`FileInner`へアクセスできる。

### 4.3 R-Logに保持する復元情報

復元元として、`File.id`ごとに1つの`RLog`を格納する`R_LOG_TABLE`を追加した。

```rust
pub enum RecoveryType {
    None,
    Inode { dev: u32, inum: u32 },
    Device { dev: u32, inum: u32, major: u16 },
}

pub struct RLog {
    pub readable: bool,
    pub writeable: bool,
    pub r#type: RecoveryType,
    pub offset: u32,
}

pub struct RLogTable {
    pub entries: SpinLock<[RLog; NFILE]>,
}
```

`RLog`は`FileInner`と同じ**論理的な状態**（権限・種類・位置）を保持するが、`FileType::Inode`の`Inode`ハンドルをそのままコピーするのではなく、`dev`と`inum`を復元用の識別情報として保存する。このため「`FileInner`をそのまま複製して保存した」と表現するのは正確ではない。メモリ消費量の削減効果は本稿では測定していない。

通常ファイルでは、`sys_open()`で`RLog`の種類・権限・初期`offset`を設定し、`read()`で読み込みに成功したとき、`write()`で各チャンクの書き込みに成功したときに`RLog.offset`を更新する。最終`close()`では該当エントリを初期化する。R-Logはカーネルのメモリ内の記録であり、ディスク上の永続的なredoログではない。

### 4.4 `replace_file_inner()`による再生成と差し替え

`FileTable::replace_file_inner(id)`の処理は次の順序である。

1. `R_LOG_TABLE.entries[id]`から`readable`、`writeable`、`RecoveryType`、`offset`を取得する。
2. `RecoveryType::Inode`または`Device`の場合、`dev`・`inum`を用いて`Inode::recovery_inode_get()`から既存のinode tableエントリを参照する。`None`または復元対象が見つからない場合はエラーを返す。
3. 復元した情報で`FileInner`を生成し、`Arc::new(SleepLock::new(...))`で新たにヒープ確保する。
4. `FILE_TABLE.inner`の`SpinLock`を取得し、対象`id`のスロットを新しい`Arc`に差し替える。
5. 旧`Arc<SleepLock<FileInner>>`を戻り値として返す。

中心となる差し替え処理は以下のとおりである。

```rust
let slot = inner_table
    .get_mut(id)
    .and_then(|entry| entry.as_mut())
    .ok_or(SysError::IoError)?;

let old_inner = core::mem::replace(slot, new_inner);
Ok(old_inner)
```

`core::mem::replace`により、テーブル上の参照は新しいオブジェクトを指す。**すでに別の箇所が保持している旧`Arc`まで、新しいオブジェクトに向き直るわけではない。** そのため、回復処理後に改めて`FILE_TABLE.get_inner(id)`を実行して、新しい`FileInner`を取得することが重要となる。

旧`Arc`を返すことで、Rust上の共有参照を関数の外で保持し、スコープの終了に伴ってdropさせられる。ただし、古いオブジェクトのメモリが破損している実際のUE環境で、安全な破棄処理まで保証できているわけではない。

### 4.5 inodeの参照数と所有権移管

`Inode`はRustの`clone()`だけでは`INODE_TABLE.meta[id].r#ref`を増やさない。通常の`Inode::get()`や`Inode::dup()`は論理的な参照数を増加させるが、回復処理でそれを行うと、元の`FileInner`が保持していた参照に加えて余分な参照が発生する可能性がある。

そこで、`Inode::recovery_inode_get(dev, inum)`は既存エントリの存在を確認し、**参照数を増やさず**に新しい`Inode`ハンドルを作る。設計上は、旧`FileInner`が担っていた1つのinode参照の解放責任を、新しい`FileInner`に引き継がせる。

この方式が成立するための前提は次のとおりである。

- 元の`FileInner`が所有していたと扱うinode参照がまだ有効である。
- 同じ参照の解放責任を旧側と新側で二重に行使しない。
- 最終`close()`時に新しい`FileInner`に対応する`Inode::put()`を一度だけ実行する。

これはRustの型システムだけで自動的に証明される所有権移動ではなく、**OS側の参照管理規約に依存する設計**である。複数の古いハンドルが存在する場合や並行した`close()`との競合は別途検証する必要がある。

### 4.6 ロックと状態更新の整合性

復元情報を保持する`R_LOG_TABLE.entries`、参照先を保持する`FILE_TABLE.inner`、および個々の`FileInner`は、別々のロックで保護される。各ロックが局所的なデータ競合を防ぐ一方、これらをまたぐ一連の操作が**単一の不可分な処理になるわけではない**。今回の基本検証では、故障注入箇所と実行条件を制限してこの問題を扱っている。

## 5. `read()` / `write()`への回復方式の適用

### 5.1 両者に共通する透過的Restoring

PR #10以降の`read()`およびPR #9の`write()`は、いずれも **`FileInner`を取得してロックする前に** 疑似UEの注入判定とRestoringを実行する。回復後の新しいオブジェクトを取得して、呼び出し元に`IoError`を返さず同じシステムコールで通常処理を続ける。

```rust
// read/writeに共通する形（実際はそれぞれ専用のENABLE_*_UEを使用）
if ENABLE_READ_UE && should_inject_file_inner_ue(self.id) {
    let _old_inner = FILE_TABLE.replace_file_inner(self.id)?;
}

let file_inner_ref = FILE_TABLE.get_inner(self.id);
let mut file_inner = file_inner_ref.lock();
```

上のコードは**`read()`の実例**である。`write()`では`ENABLE_WRITE_UE`を用いる。回復関数`replace_file_inner()`自体は両者に共通する。

```text
read() / write() 呼び出し
        │
        ▼
アクセス前の疑似UE判定
        │ UE条件成立
        ▼
R-LogからFileInnerを再生成
        │
        ▼
FILE_TABLE.inner[id]を差し替え
        │
        ▼
新しいFileInnerを取得・ロック
        │
        ▼
readable / writeableを確認
        │
        ▼
通常のinode.read() / inode.write()へ進む
        │
        ▼
成功時にFileInner.offsetとRLog.offsetを更新
```

### 5.2 `read()`への適用

最初の`read()`回復実装では、旧`FileInner`のガードを取得した後にRestoringし、`IoError`を返していた。そのため、アプリケーション側で次の`read()`を発行する必要があった。現在は注入判定と差し替えをロック取得前に移したため、回復後の新しい`FileInner`を利用して**同一の`read()`で読み込みを継続**できる。

通常ファイルの読み込み成功後は、取得バイト数に応じて`FileInner.offset`を進め、続いて`RLog.offset`も更新する。これにより、後続の疑似UEで必要となる読み込み位置を保持する。

### 5.3 `write()`への適用

`write()`も同じ`replace_file_inner()`で復元後に新しい`FileInner`を取得し、同一システムコールで書き込みを継続する。`write()`固有の処理として、複数チャンクへの分割とファイルシステムログ操作がある。現行実装では、各チャンクのinode書き込みおよびログ操作終了後、成功分の`FileInner.offset`を更新し、その値を`RLog.offset`に反映する。

今回の疑似UEは **`write()`のデータ更新を始める前** に注入される。このため、現在の実験では復元後に書き込みを最初から実施できる。一方、inodeやデータブロックの更新途中にUEが発生した場合にどこから再開するか、部分書き込みをどう扱うかという問題はまだ解決していない。

### 5.4 共通点と相違点

| 観点                 | `read()`                          | `write()`                          |
| -------------------- | --------------------------------- | ---------------------------------- |
| UE判定箇所           | `readable`へのアクセス前          | `writeable`へのアクセス前          |
| 回復対象             | 同一の`FileInner`                 | 同一の`FileInner`                  |
| 回復関数             | `replace_file_inner()`            | `replace_file_inner()`             |
| 回復後               | 新しい`FileInner`で同一readを継続 | 新しい`FileInner`で同一writeを継続 |
| 成功時の`offset`更新 | 読み込んだバイト数                | 成功したチャンクの書き込みバイト数 |
| 固有の注意点         | EOFと読み込み位置の維持           | ログトランザクションと部分書き込み |

## 6. 動作検証

### 6.1 検証の前提

故障注入は`kernel/src/file.rs`の`ENABLE_READ_UE`または`ENABLE_WRITE_UE`を有効化したときにのみ実行される。対象は`UE_TARGET_ID=1`で、`should_inject_file_inner_ue()`のカウンタが所定の値になった呼び出しで注入される。カウンタはカーネル全体で共有されるため、検証時にはテスト用に起動し直した環境、対象`File.id`、注入タイミングを確認する必要がある。

**両フラグが`false`の通常ビルドでは、テストプログラムが成功してもRestoring自体が実行された証拠にはならない。** したがって、故障注入を有効化した状態のDEBUGログと、ユーザプログラムの結果を組み合わせて判断する。

### 6.2 `read()`の検証

テストファイルの内容を`recovery test\n`とした上で、[`read-test.rs`](https://github.com/IbukiShinzato/octopos/blob/e4972dbaf3dc1653d93b7a7bdb5c5b67fdd53f27/user/bin/read-test.rs)で次の動作を検証した。

| 回数 | 動作                     | 期待値              | `offset` |
| ---- | ------------------------ | ------------------- | -------- |
| 1    | 通常read                 | `recovery`、8バイト | 0 → 8    |
| 2    | 疑似UE後、同一readで継続 | ` test\n`、6バイト  | 8 → 14   |
| 3    | 後続read                 | EOF、`Ok(0)`        | 14のまま |

実験で記録したカーネルログは次のとおりである。

```text
DEBUG: UE injected in read()
DEBUG: old_inner = 0x87e4a010
DEBUG: old offset = 8
DEBUG: new_inner = 0x87fc1010
DEBUG: restored offset = 8
TEST: read recovery test passed
```

別途、inode参照数について以下を確認した。

```text
DEBUG: Before Restoring: Inode.ref = 1, offset = 8
DEBUG: After Restoring: Inode.ref = 1, offset = 8
DEBUG: After close: Inode.ref = 0
TEST: read recovery test passed
```

旧・新`FileInner`のアドレスが異なり、`offset=8`が新しいオブジェクトに復元されている。また、同じFDで2回目のreadが成功し、3回目がEOFを返していることから、**今回の単一故障注入条件での透過的な読み込み継続**を確認できた。inode参照数もRestoring前後で1を維持し、最終`close()`後に0となった。

### 6.3 `write()`の検証

[`write-test.rs`](https://github.com/IbukiShinzato/octopos/blob/e4972dbaf3dc1653d93b7a7bdb5c5b67fdd53f27/user/bin/write-test.rs)は、同じFDで`hello`、`world`、`!`を順に書き込み、close後に開き直して`helloworld!`（11バイト）を確認する。疑似UEを2回目の対象writeに注入した場合、`offset=5`からRestoringし、同じwrite内で`world`を書き込んで、その後の`!`まで継続する設計である。

[`write-4kib.rs`](https://github.com/IbukiShinzato/octopos/blob/e4972dbaf3dc1653d93b7a7bdb5c5b67fdd53f27/user/bin/write-4kib.rs)では、4096バイトの`a`を書いた後に`!`を追記し、再open後に4097バイトの内容を検証する。4096バイトのカーネル内部書き込みは3072バイトと1024バイトに分割される。実験時には`offset=4096`からの復元、追記後の`offset=4097`、最終close後のinode参照数が0になることを確認している。

| テスト          | 検証内容                       | 確認した結果                                 |
| --------------- | ------------------------------ | -------------------------------------------- |
| `write-test.rs` | 3回の連続write・同一FDでの継続 | `helloworld!`（11バイト）を再open後に確認    |
| `write-4kib.rs` | 複数チャンク更新・復元後の追記 | 4096バイト＋1バイトの内容を確認              |
| inode参照数     | 回復前後と最終close            | 回復で不必要に増加せず、対象の最終close後に0 |

これらはこれまでの手動実験・実行報告に基づく。GitHub Actionsのビルド成功だけでは故障注入時のカーネル実行を検証できない。再現可能性の向上には、故障注入の発生回数と回復実行を自動assertする仕組みが必要である。

## 7. 考察と制約

### 7.1 Rustの特徴と回復設計の関係

変更前の`FileInner`は固定配列内に直接格納されており、テーブル上の参照先を交換する方式には向いていなかった。これを`Arc<SleepLock<FileInner>>`へ変更することで、個別のオブジェクトをヒープに確保し、テーブル上の参照を新オブジェクトへ切り替えられるようにした。

ただし、`Arc`は**複数の所有者による共有**を実現するものであって、オブジェクト破損からの回復機能そのものを提供するものではない。旧`Arc`を保持した処理が残る問題、Inodeの論理的な参照数、ロックの取得順序、復元情報の整合性は、OS設計側で扱う必要がある。

また、RAIIによるロックの自動解放と、Inode参照数の明示的な`put()`は別の管理機構である。今回の`recovery_inode_get()`は後者を考慮し、既存参照の解放責任を新しい`FileInner`へ引き継ぐ方式を採用した。

### 7.2 現段階で主張できること

- `FileInner`へのアクセス前に疑似UEを注入する限定条件では、R-Logからオブジェクトを再生成してテーブル上の参照を差し替えられた。
- 復元後に新しい`FileInner`を取得することにより、`read()`と`write()`の両方で**同一システムコール内の処理継続**を確認した。
- 基本テストでは`offset`と読み書き内容を維持し、単一FDのinode参照数が期待値を保つことを確認した。

### 7.3 現段階で保証していないこと

1. **実ハードウェアUE**：ECCのUE通知、故障物理ページの隔離、故障メモリへの安全な非アクセス、旧領域の安全な解放は未検証である。
2. **アクセス途中の故障**：読み込みや書き込みの途中で例外が発生した場合の再開位置・部分操作の扱いは未実装である。
3. **並行処理**：旧`Arc`を保持する別の処理、競合する`close()`、多CPU環境での安全性は検証していない。
4. **R-Logの整合性**：`FileInner.offset`と`RLog.offset`の更新が同一の原子的操作ではない。両者の更新途中にUEが発生した場合の扱いは未解決である。
5. **ログの永続性・トランザクション整合性**：R-Logはメモリ内の回復用記録であり、ファイルシステムの物理redoログとは独立している。両者の原子的なコミットは保証していない。
6. **対象の一般化**：通常ファイル以外の`FileType`、複数回の故障注入、多様なファイル共有状態について十分な評価は行っていない。
7. **テスト自動化**：故障注入フラグが無効でも既存の読み書きテストは通る。テスト成功のみで実際のRestoring実行を判定できない。

### 7.4 今後の作業候補

今後は、回復可能な故障時点を明確にした上で、複数回注入・複数FD・共有参照のシナリオを拡充する。また、故障発生の有無を自動的に記録・検証する仕組み、R-Logの更新整合性、部分書き込み後の再開方針、実UEに近い条件での安全性を検討する必要がある。

## 参照したソースコード・PR

- **回復前の比較基準**：[`kernel/src/file.rs`（`0e92fd6`）](https://github.com/IbukiShinzato/octopos/blob/0e92fd67885d8517393c659fa3b6da1815d4076e/kernel/src/file.rs)
- **回復実装後の確認基準**：[`kernel/src/file.rs`（`e4972db`）](https://github.com/IbukiShinzato/octopos/blob/e4972dbaf3dc1653d93b7a7bdb5c5b67fdd53f27/kernel/src/file.rs)
- [`kernel/src/fs.rs`](https://github.com/IbukiShinzato/octopos/blob/e4972dbaf3dc1653d93b7a7bdb5c5b67fdd53f27/kernel/src/fs.rs) — `Inode`・`InodeTable`・参照管理
- [`kernel/src/sysfile.rs`](https://github.com/IbukiShinzato/octopos/blob/e4972dbaf3dc1653d93b7a7bdb5c5b67fdd53f27/kernel/src/sysfile.rs) — `sys_read()`・`sys_write()`・`sys_open()`
- [`kernel/src/log.rs`](https://github.com/IbukiShinzato/octopos/blob/e4972dbaf3dc1653d93b7a7bdb5c5b67fdd53f27/kernel/src/log.rs) — ファイルシステムのトランザクション
- [`user/src/io.rs`](https://github.com/IbukiShinzato/octopos/blob/e4972dbaf3dc1653d93b7a7bdb5c5b67fdd53f27/user/src/io.rs) — `Read`・`Write`トレイト
- [PR #8：read回復の導入](https://github.com/IbukiShinzato/octopos/pull/8)、[PR #9：write回復](https://github.com/IbukiShinzato/octopos/pull/9)、[PR #10：readの透過的回復](https://github.com/IbukiShinzato/octopos/pull/10)
