# octoposにおけるopen/closeシステムコールとオブジェクト管理の調査

## 1. 調査目的

本調査では、Rust製OSであるoctoposの`open()`と`close()`が、ファイルディスクリプタ（FD）、`File`、`FileInner`および`Inode`をどのように生成・参照・解放するのかを整理する。あわせて、C版xv6との比較から、Rustの所有権・借用とRAIIが資源管理にどのように現れるかを確認する。

今回の研究では`FileInner`を回復対象としているため、特に、**`FileInner`の生成時に何が初期化され、最終close時にどの参照が解放されるか**を明らかにする。

## 2. open()の処理経路

### 2.1 ユーザー空間からカーネル空間まで

ユーザー空間からの典型的な呼び出し経路は次のとおりである。

```text
アプリケーション（例：user/bin/cat.rs）
    open(path, OpenFlag::READ_ONLY)
         ↓
user/src/syscall.rs
    open(path, flags)
         ↓
    raw::open(cpath.as_ptr(), flags)
         ↓
    syscall2(Syscall::Open, path, flags)
         ↓
    ecall（ユーザー空間からカーネル空間への遷移）
         ↓
kernel/src/syscall.rs
    Syscall::Open => sys_open(&args)
         ↓
kernel/src/sysfile.rs
    sys_open(&args)
```

ユーザー空間の`open()`では、パス文字列を検証して、終端NUL付きのバッファに変換した後、システムコールを発行する。`syscall2`は第1引数と第2引数をレジスタに設定し、`ecall`を実行する。

```rust
// user/src/syscall.rs（要点）
pub fn open(path: &str, flags: usize) -> Result<Fd, SysError> {
    let cpath = validate_path(path)?;
    check(raw::open(cpath.as_ptr(), flags)).map(Fd)
}

// rawモジュール
pub fn open(path: *const u8, flags: usize) -> isize {
    syscall2(Syscall::Open, path as usize, flags)
}
```

`sys_open()`は、システムコール引数の0番目から**ユーザー空間にあるパス文字列のアドレス**を取得して文字列を読み込み、`Path`を生成する。1番目の引数からはオープンフラグを取得する。0番目の引数が直接`Path`型で渡されるわけではない。

### 2.2 sys_open()によるInodeの取得

`sys_open()`では、まず`Operation::begin()`を呼び出してファイルシステムのログトランザクションを開始する。次に`OpenFlag::CREATE`の有無によって処理を分岐する。

- `CREATE`を指定した場合：`Inode::create()`を呼び出し、ファイルの作成または既存ファイルの取得を行う。
- `CREATE`を指定しない場合：`Path::resolve()`で既存ファイルに対応する`Inode`を取得し、`inode.lock()`で`InodeInner`のロックガードを取得する。

ディレクトリを読み取り専用以外で開こうとした場合は`SysError::IsDirectory`を返す。また、デバイスファイルのmajor番号が許容範囲外の場合は`SysError::NoEntry`を返す。**ディレクトリやデバイスが常にエラーになるわけではない。**

### 2.3 FileとFDの確保

```rust
// kernel/src/sysfile.rs（要点）
let (fd, file) = match log!(File::alloc()) {
    Ok(mut file) => match log!(fd_alloc(file.clone())) {
        Ok(fd) => (fd, file),
        Err(e) => {
            file.close();
            inode.unlock_put(inode_inner);
            return Err(e);
        }
    },
    Err(e) => {
        inode.unlock_put(inode_inner);
        err!(SysError::from(e));
    }
};
```

ここでは、`File::alloc()`と`fd_alloc()`が**異なる資源**を確保している点が重要である。

| 処理            | 確保するもの                             | 管理場所                   |
| --------------- | ---------------------------------------- | -------------------------- |
| `File::alloc()` | カーネルのファイルオブジェクトのスロット | グローバルな`FILE_TABLE`   |
| `fd_alloc()`    | プロセス内で使用するFD番号               | プロセスの`open_files[fd]` |

`File::alloc()`は`FILE_TABLE.meta`で`ref_count == 0`のスロットを探し、`ref_count`を1にして、`File { id }`を返す。現行実装では、同時に`Arc<SleepLock<FileInner>>`を生成して、`FILE_TABLE.inner[id]`へ登録する。

`fd_alloc(file)`は`File`を値渡しで受け取り、プロセスの`open_files`の未使用要素に格納し、FD番号を返す。**FD番号と`File.id`は別の番号であり、必ずしも一致しない。**

```rust
// kernel/src/file.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub id: usize,
}
```

`File`自身はロックガードやファイルの状態を保持せず、`FILE_TABLE`へのインデックスだけを持つハンドルである。

### 2.4 FileInnerとR-Logの初期化

FDの確保後、`sys_open()`は`FILE_TABLE.get_inner(file.id)`で`Arc<SleepLock<FileInner>>`を取得し、`lock()`によって`FileInner`の可変アクセスに必要なガードを取得する。

```rust
// kernel/src/sysfile.rs（要点）
let file_inner_ref = FILE_TABLE.get_inner(file.id);
let mut file_inner = file_inner_ref.lock();
let mut r_logs = R_LOG_TABLE.entries.lock();
let r_log = &mut r_logs[file.id];
```

`InodeType`に応じて`FileType::Inode`または`FileType::Device`を設定し、オープンフラグに基づいて`readable`と`writeable`を設定する。通常ファイルの場合は`offset`を0にする。`TRUNCATE`が指定されており、対象が通常ファイルの場合は`inode.trunc()`も実行する。

今回追加した回復機構では、**同じ`File.id`に対応するR-Logも初期化する。** R-Logにはアクセス権、offset、およびInodeの識別情報（dev・inum、デバイスの場合はmajor）を保存する。`FileInner`がInodeハンドルを保持するのに対し、R-Logは復元に使う識別情報を保持する。

`sys_open()`終了前に`inode.unlock(inode_inner)`でガードを渡してロックを解放し、呼び出し元へFD番号を返す。

### 2.5 open()の失敗時の解放処理

`File::alloc()`に失敗した場合は、既に取得したInodeを`inode.unlock_put(inode_inner)`で解放する。

`File::alloc()`が成功し、`fd_alloc()`に失敗した場合は、`file.close()`で確保済みのFileスロットを解放した上で、`inode.unlock_put(inode_inner)`を呼ぶ。

このように、Rustの所有権・RAIIを利用していても、**FileやInodeの論理的な参照の解放は明示的な操作に依存している。**

## 3. close()の処理経路

> この章は元の下書きでは未記入だったため、octoposの現行`sys_close()`と`File::close()`を確認して追加した。

### 3.1 ユーザー空間からの呼び出し

```text
ユーザー空間のclose(fd)
    ↓
raw::close(fd)
    ↓
syscall1(Syscall::Close, fd)
    ↓
ecall
    ↓
sys_close(&args)
    ↓
File::close()
```

`sys_close()`は`args.get_file(0)`からFD番号と対応する`File`ハンドルを取得する。次に、プロセスの`open_files[fd]`を`None`に変更し、`File::close()`を実行する。

```rust
// kernel/src/sysfile.rs
pub fn sys_close(args: &SyscallArgs) -> Result<usize, SysError> {
    let (fd, mut file) = try_log!(args.get_file(0));

    let (_proc, data) = current_proc_and_data_mut();
    data.open_files[fd] = None;
    file.close();

    Ok(0)
}
```

### 3.2 File::close()と参照数

`File::close()`は`FILE_TABLE.meta`のSpinLockを取得し、対象の`FileMeta.ref_count`を1減らす。この参照数がまだ1以上であれば、他にもFileを利用するFDなどがあるため、ファイルオブジェクト自体の解放には進まず戻る。

最後の参照で`ref_count`が0になった場合は、`FILE_TABLE.take_inner(self.id)`で`Arc<SleepLock<FileInner>>`をテーブルから取り出し、`FileInner`の状態を複製した後、元のオブジェクトの`FileType`を`None`に戻す。続いてR-Logを初期状態へ戻し、ロックを解放してから、複製した状態の種別に応じた終了処理を行う。

```rust
// kernel/src/file.rs（要点。ロック取得などは省略）
meta.ref_count -= 1;
if meta.ref_count > 0 {
    return;
}

let file_inner_ref = FILE_TABLE.take_inner(self.id);
let mut inner = file_inner_ref.lock();
let inner_copy = inner.clone();
inner.r#type = FileType::None;
// 実装ではここでガードを解放してR-Logを初期化する

match inner_copy.r#type {
    FileType::None => {}
    FileType::Pipe { pipe } => pipe.close(inner_copy.writeable),
    FileType::Inode { inode } | FileType::Device { inode, .. } => {
        let _op = Operation::begin();
        inode.put();
    }
}
```

※上記は**処理の概略を示す抜粋**であり、実際のコードではロックガードを明示的にdropしてから`inode.put()`等を実行する。完全な処理順序は`kernel/src/file.rs`を参照する。

`inode.put()`は`INODE_TABLE.meta`の参照数を減らす。最後の参照であり、かつファイルシステム上のリンク数が0などの条件が成立すれば、Inodeの解放処理に進む。この処理をログトランザクション内で行うのは、Inodeの解放にディスク更新が伴う場合があるためである。

したがって、**FDを閉じること、`FileMeta.ref_count`が0になること、`Arc`の強参照数が0になること、`InodeMeta.r#ref`が0になることは、それぞれ異なる出来事**である。

## 4. メモリオブジェクトと参照管理の整理

| オブジェクト・情報          | 役割                                     | 主な管理方法                      |
| --------------------------- | ---------------------------------------- | --------------------------------- |
| `Path`                      | パスを表現・探索するための値             | Rustの値・借用                    |
| `Fd`                        | ユーザー空間のファイルディスクリプタ番号 | ユーザー空間の値                  |
| `open_files[fd]`            | プロセスのFDから`File`への対応           | プロセスごとの配列                |
| `File { id }`               | FileTableを指すハンドル                  | `File.id`による識別               |
| `FileMeta`                  | Fileオブジェクトの論理的参照数           | `SpinLock`で保護                  |
| `Arc<SleepLock<FileInner>>` | 共有可能なFileInnerとその排他制御        | `Arc`の強参照数・`SleepLock`      |
| `FileInner`                 | readable・writeable・type・offset        | `SleepLock`で保護                 |
| `Inode`                     | Inode tableを参照するハンドル            | id・dev・inum                     |
| `InodeInner`                | キャッシュされたInodeの可変状態          | `SleepLock`で保護                 |
| `InodeMeta`                 | Inode tableの論理的参照数                | `SpinLock`で保護                  |
| `RLog`                      | FileInnerの復元に必要な情報              | `R_LOG_TABLE.entries`の`SpinLock` |

### 4.1 3種類の参照数

次の3種類は別々に管理される。

1. **`FileMeta.ref_count`**：`File::alloc()`で1となり、`File::dup()`で増え、`File::close()`で減る。単なる`File::clone()`では増えない。
2. **`Arc`の強参照数**：`FILE_TABLE.get_inner()`の`Arc::clone()`などで増え、保持する`Arc`がdropされると減る。`FileMeta.ref_count`と一致するとは限らない。
3. **`InodeMeta.r#ref`**：`Inode::get()`や`Inode::dup()`で増え、`Inode::put()`で減る。`Inode::clone()`はInodeハンドルの値を複製するだけで、この参照数を増やさない。

このため、**Rustの`Clone`や`Arc`による所有権の管理だけでは、OS独自のInode参照数の整合性は自動的に保証されない。**

### 4.2 ロックとRAII

`FILE_TABLE.meta`や`R_LOG_TABLE.entries`はSpinLockによって保護される。`FileInner`や`InodeInner`はSleepLockによって保護され、競合したプロセスをスリープさせられる。ロックを選ぶ基準はフィールド数ではなく、保持期間や待機中にCPUを消費し続けることが適切かどうかである。

octoposの`SleepLockGuard`や`SpinLockGuard`は、ガードのdropによってロックを解放する。`inode.unlock(guard)`も内部ではガードをdropする処理である。したがって、**unlock関数を明示的に呼んでいることと、RAIIを利用していることは矛盾しない。**

また、`Operation::begin()`が返す`Operation`はdrop時に`end_op()`を呼び出す。このため、ログトランザクションの終了処理にはRAIIが利用される。ただし、RAIIはデッドロックを防止するものではない。

## 5. C版xv6との比較

元の下書きで取り上げたC版xv6の`sys_open()`の一部と、octoposの`sys_open()`を比較する。

### 5.1 FileとFDの割り当て

**C版xv6**（`kernel/sysfile.c`、元の調査で引用した部分）：

```c
if ((f = filealloc()) == 0 || (fd = fdalloc(f)) < 0) {
    if (f)
        fileclose(f);
    iunlockput(ip);
    end_op();
    return -1;
}
```

**octopos**（`kernel/src/sysfile.rs`）：

```rust
let (fd, file) = match log!(File::alloc()) {
    Ok(mut file) => match log!(fd_alloc(file.clone())) {
        Ok(fd) => (fd, file),
        Err(e) => {
            file.close();
            inode.unlock_put(inode_inner);
            return Err(e);
        }
    },
    Err(e) => {
        inode.unlock_put(inode_inner);
        err!(SysError::from(e));
    }
};
```

どちらも、グローバルなファイルオブジェクトを確保し、続いてプロセスにFDを割り当てるという基本構造は共通している。一方、C版xv6の`filealloc()`は`struct file *`を返し、octoposの`File::alloc()`は`File { id }`という値を返す。

octoposの`fd_alloc()`は`File`の所有権を受け取るため、呼び出し元でもハンドルを使い続けるには`file.clone()`が必要となる。ただし、**`clone()`は論理的なファイル参照数を増やさない。** `FileMeta.ref_count`の更新は`File::alloc()`・`File::dup()`・`File::close()`などが担当する。

### 5.2 ロックとトランザクションの管理

| 観点                     | C版xv6                         | octopos                                           |
| ------------------------ | ------------------------------ | ------------------------------------------------- |
| ファイルハンドル         | `struct file *`                | `File { id }`                                     |
| オブジェクトへのアクセス | ポインタ経由でロックを明示操作 | `File.id`で対象を求め、ロックガード経由でアクセス |
| エラー表現               | 典型的には負の値を返す         | `Result<_, SysError>`を用いる                     |
| トランザクション開始     | `begin_op()`                   | `Operation::begin()`                              |
| トランザクション終了     | `end_op()`を明示的に呼ぶ       | `Operation`のdropから`end_op()`を呼ぶ             |
| Inode参照の解放          | `iput()`／`iunlockput()`など   | `inode.put()`／`inode.unlock_put()`など           |

Rustの所有権・借用やガードは、資源管理の一部を型や寿命と結びつける。一方で、Inodeの論理的参照数のように明示的に整合性を保つ必要がある資源も残っている。

## 6. 回復方式との関係・考察

`open()`は、読み書きのための`FileInner`を初期化するとともに、同じ`File.id`に対応するR-Logを初期化する。`read()`と`write()`は、正常に読み書きできた場合にoffsetおよびR-Log上のoffsetを更新する。最終`close()`では、R-Logを初期化状態に戻し、Inodeに対する論理的参照を解放する。

今回のRestoringでは、R-Logを元に新しい`FileInner`を生成し、`FILE_TABLE.inner`上の`Arc`を差し替える。一方、Inodeについては既存の論理的参照を新しいハンドルへ引き継ぐ必要があるため、Inode参照数を無条件に増やすことはできない。ここでは`Inode::recovery_inode_get()`を用いて参照数を増加させずにハンドルを再構築している。

これはRust固有の参照管理によって自動的に解決された問題ではなく、**Rustの`Arc`とOS独自の参照数が別々であることを踏まえ、プログラマが整合性を設計した部分**である。同様の論理的参照管理はC版xv6にも存在するため、この事実だけで学術的新規性が示されたとは言えない。

また、テーブル上の`Arc`を差し替えても、既に別の処理が保持している古い`Arc`が新しいオブジェクトを指すようになるわけではない。現在のread/writeでの疑似故障注入は、`FileInner`を取得する前に実行する設計である。この条件と、複数の参照が存在する条件を混同しないことが重要である。

### 今後確認する点

- `dup()`で複数FDが同じFileオブジェクトを参照している場合の`close()`とRestoringの整合性。
- 既存の`Arc`を保持した処理とRestoringが同時に実行される場合の安全性。
- R-Logと`FileInner`の状態更新途中で故障した場合の整合性。
- 実際のUE発生時に、破損領域へのアクセスや解放を安全に扱えるか。

## 7. 参照したソースコード

- [octopos: `user/src/syscall.rs`](https://github.com/IbukiShinzato/octopos/blob/main/user/src/syscall.rs)
- [octopos: `kernel/src/syscall.rs`](https://github.com/IbukiShinzato/octopos/blob/main/kernel/src/syscall.rs)
- [octopos: `kernel/src/sysfile.rs`](https://github.com/IbukiShinzato/octopos/blob/main/kernel/src/sysfile.rs)
- [octopos: `kernel/src/file.rs`](https://github.com/IbukiShinzato/octopos/blob/main/kernel/src/file.rs)
- [octopos: `kernel/src/fs.rs`](https://github.com/IbukiShinzato/octopos/blob/main/kernel/src/fs.rs)
- [octopos: `kernel/src/sleeplock.rs`](https://github.com/IbukiShinzato/octopos/blob/main/kernel/src/sleeplock.rs)
- [octopos: `kernel/src/log.rs`](https://github.com/IbukiShinzato/octopos/blob/main/kernel/src/log.rs)
- [xv6-riscv: `kernel/sysfile.c`](https://github.com/mit-pdos/xv6-riscv/blob/riscv/kernel/sysfile.c)（元の下書きで引用されたコード片）
