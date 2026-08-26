# Rustらしさの調査

## 1. 調査目的

Rust版xv6であるOctoposが、所有権・借用・型システム・RAIIなどのRustの特徴を活かした実装になっているかを調査する。

また、研究テーマとして検討している「Rustで実装されたOSのレジリエンス性強化」において、Octoposが対象OSとして適しているかを判断するため、xv6のC実装とOctoposのRust実装を比較する。

今回は主に`read()`、`write()`、`exec()`、`fork()`を対象とする。

## 2. 評価観点

### 2.1 型安全性

Octoposでは、意味の異なる値を別の型として表現している。

例えば、仮想アドレスと物理アドレスはそれぞれ`VA`、`PA`として定義されている。そのため、単純な整数としてアドレスを扱う場合と比較して、仮想アドレスと物理アドレスを誤って混同する処理を型エラーとして検出できる。

また、xv6では多くの関数がエラー時に`-1`を返すのに対し、Octoposでは次のような型を使用している。

```rust
Result<usize, SysError>
Result<Pid, KernelError>
Result<usize, ExecError>
```

これにより、成功時の値とエラーを型として分離できる。

### 2.2 所有権・借用

Octoposでは、構造体内部のデータを直接自由に操作するのではなく、参照や可変参照を利用してアクセスする。

また、借用中のデータに対して再度競合する借用を行おうとすると、borrow checkerによってコンパイル時に検出される。

例えば`File::write()`では、次の`clone()`を削除するとborrow checkerによるエラーが発生した。

```rust
let inode = inode.clone();
```

`match &mut file_inner.r#type`によって`file_inner.r#type`が可変借用されている状態で、その内部の`inode`をそのまま利用すると、`file_inner.offset`へ再度アクセスする際に借用が競合する。

`inode.clone()`によってローカルに`Inode`値を作ることで、`file_inner.r#type`内部への借用から切り離して処理している。

ただし、`Inode::clone()`はinode tableの参照カウントを増やさない。OS内部のinode参照を新しく所有する場合には`dup()`を利用する必要がある。

```rust
pub fn dup(&self) -> Self {
    let mut meta = INODE_TABLE.meta.lock();
    meta[self.id].r#ref += 1;
    self.clone()
}
```

このように、Rustの所有権とは別に、OS独自の参照カウントを管理する必要がある場合も存在する。

### 2.3 RAIIと資源管理

Octoposではlockを取得するとguardが返され、guardがdropされることでlockが解放される。

例えば、

```rust
{
    let mut parents = PROC_TABLE.parents.lock();
    parents[new_proc.id] = Some(proc.id);
}
```

では、ブロックを抜けた時点でlock guardがdropされ、lockが解放される。

この仕組みにより、Cのように、

```c
acquire(&wait_lock);

/* processing */

release(&wait_lock);
```

と明示的に`release()`を書く場合と比較して、unlockの書き忘れを防ぎやすい。

一方で、RAIIによってデッドロックそのものが防げるわけではない。

`File::read()`でinode lockを保持したまま、同じinodeに対して再度lockを取得する実験を行った。

```rust
let mut inode_inner = inode.lock();

println!("before second lock");

let _other_inner = inode.lock();

println!("after second lock");
```

この場合、

```text
before second lock
```

までは出力されるが、2回目の`inode.lock()`で待機し続けるため、

```text
after second lock
```

には到達しない。

したがって、

> RAIIはunlockの書き忘れを防ぐことには有効だが、lockをどの範囲で保持するかという寿命設計はプログラマ自身が行う必要がある。

と考えられる。

また、すべての資源がRAIIによって自動解放されているわけではない。例えばPageTableやinodeの独自参照カウントなど、一部の資源については明示的な解放処理が必要である。

### 2.4 エラー処理

xv6では、多くのカーネル関数が失敗時に`-1`を返す。

一方、Octoposでは、

```rust
SysError
ExecError
FsError
VmError
KernelError
```

など、処理のレイヤごとにエラー型が定義されている。

例えば`File::read()`は、

```rust
pub fn read(&self, addr: VA, n: usize) -> Result<usize, SysError>
```

となっており、読み込み不可能なfile descriptorでは、

```rust
Err(SysError::BadDescriptor)
```

を返す。

このように、単純な`-1`だけではなく、失敗理由を型として保持できる点がOctoposの特徴である。

### 2.5 unsafe境界

OSではハードウェアや生ポインタを扱うため、Rustであっても`unsafe`を完全に排除することは難しい。

Octoposでは、生ポインタからsliceを構築する場合などに`unsafe`を明示している。

例えば`File::read()`では、

```rust
let dst = unsafe {
    slice::from_raw_parts_mut(addr.as_mut_ptr(), n)
};
```

として、仮想アドレスから取得した生ポインタを`&mut [u8]`へ変換している。

また`exec()`では、

```rust
// # Safety: we are the current proc
let data = unsafe { proc.data_mut() };
```

として、`ProcData`への直接的な可変参照を取得している。

ここでは「現在実行しているprocess自身であり、排他的にアクセスできる」という不変条件をプログラマ側で保証している。

したがってOctoposでは、

```text
unsafeな低レイヤ操作
        ↓
Rustの参照・sliceなどへ変換
        ↓
以降をsafe Rustで処理
```

という構造が見られる。

ただし、safe Rustのみで書かれたコードであっても、デッドロックなどの論理的・並行処理上のバグは発生する。

## 3. read / write

ユーザーランド側の`read()`では、

```rust
pub fn read(fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
    check(raw::read(fd.as_raw(), buf.as_mut_ptr(), buf.len()))
}
```

となっている。

ユーザーが利用するsafe APIでは、

```rust
&mut [u8]
```

を利用し、raw syscall境界で初めて、

```text
pointer + length
```

へ変換している。

そのため、

```text
Safe Rust API
    &mut [u8]
        ↓
Raw syscall ABI
    *mut u8 + usize
```

という境界が存在する。

カーネル側では`FileType`がenumとして定義され、`Pipe`、`Inode`、`Device`などに応じて処理が分岐する。

Rustではvariantと、そのvariantに必要なデータをまとめて表現できるため、単純な整数tagによって状態を管理する場合よりも、状態と保持データの対応関係を型として表現できる。

また、戻り値も、

```rust
Result<usize, SysError>
```

であり、xv6の`-1`とは異なりエラー内容を型として表している。

### lockの実験

inode lockを保持した状態でもう一度同じlockを取得すると自己デッドロックが発生した。

このことから、RustのRAIIによってunlock忘れは防ぎやすくなるものの、critical sectionの範囲そのものはプログラマが適切に設計する必要があることが分かった。

### borrow checkerの実験

`File::write()`から、

```rust
let inode = inode.clone();
```

を削除すると、

```text
E0502: cannot borrow as immutable because it is also borrowed as mutable
E0499: cannot borrow as mutable more than once at a time
```

が発生した。

これは、`match &mut file_inner.r#type`による借用と、`file_inner.offset`へのアクセスが競合するためである。

このような競合をコンパイル時に検出できることは、Rustによる安全性の具体例である。

## 4. exec

ユーザーランドでは、

```rust
pub fn exec(path: &str, argv: &[&str]) -> SysError
```

というsafe APIを利用する。

一方raw syscallでは、

```rust
pub fn exec(path: *const u8, argv: *const *const u8) -> isize
```

となっている。

つまり、

```text
&str / &[&str]
        ↓
raw pointer
        ↓
syscall
```

という境界になっている。

### ユーザー引数の取得

`sys_exec()`では、ユーザー空間に存在する`argv[i]`のポインタ値を一度ローカル変数`uarg`へコピーする。

```rust
let mut uarg: usize = 0;

let dst = unsafe {
    slice::from_raw_parts_mut(
        &mut uarg as *mut usize as *mut u8,
        size_of::<usize>(),
    )
};

data.pagetable_mut()
    .copy_from(uargv + i * size_of::<usize>(), dst);
```

ここでは`uarg`自身のメモリを一時的に`&mut [u8]`として扱い、`copy_from()`によってユーザー空間のポインタ値を書き込んでいる。

その後、

```rust
let s = args.fetch_string(VA::from(uarg), PGSIZE)?;
argv_bufs.push(s);
```

のように、ユーザー空間の文字列をカーネル側の`String`として取得する。

Rust版では、

```rust
Vec<String>
```

が文字列そのものを所有し、

```rust
let argv: Vec<&str> =
    argv_bufs.iter().map(|s| s.as_str()).collect();
```

によって`String`への参照を作って`exec()`へ渡している。

C版でもユーザー空間の文字列をカーネルメモリへコピーするが、`kalloc()`した領域を最後に`kfree()`する必要がある。

Rustでは`Vec<String>`がスコープを抜けることで自動的に解放されるため、この部分ではRAIIによる資源管理が利用されている。

### ExecError

Rust版では、

```rust
Result<usize, ExecError>
```

のように、ELFヘッダ不正、メモリ確保失敗などの失敗をエラー型として表現する。

一方xv6では、多くのエラー経路が最終的に`-1`となる。

### PageTableの解放

xv6ではエラー発生時に、

```c
goto bad;
```

としてcleanup処理を一か所へ集約している。

Octoposでは、エラーが発生する場所ごとに、

```rust
pagetable.proc_free(size);
```

などを呼び出している。

そのため、RustであってもPageTableのように明示的な解放が必要な資源は存在する。

この点については、C版の`goto bad`によるcleanupの方が一か所に処理がまとまっており、見通しが良い部分もある。

### unsafe

`exec()`では次のような処理も存在する。

```rust
let ustack_ptr = unsafe {
    slice::from_raw_parts(
        ustack.as_ptr() as *const u8,
        (argc + 1) * size_of::<u64>(),
    )
};
```

これはユーザーstack pointerを変換しているのではなく、カーネル側の`[u64]`配列をbyte sliceとして再解釈している。

Cでは、

```c
(char *)ustack
```

のようなcastで行う処理を、Rustでは`unsafe`として明示している。

### PageTableの所有権

新しいprogram imageを構築した後、

```rust
let old_pagetable =
    data.pagetable.replace(pagetable).unwrap();
```

として新しいPageTableをprocessへ渡している。

ここでは、

```text
ローカル変数が新しいPageTableを所有
        ↓
ELFのロードに成功
        ↓
ProcDataへ所有権を移動
        ↓
古いPageTableを取得
        ↓
古いPageTableを解放
```

という流れになっている。

これは、Rustの所有権によってprocessのaddress spaceの入れ替えを表現している例と考えられる。

## 5. fork

Rust版のkernel `fork()`は、

```rust
pub fn fork() -> Result<Pid, KernelError>
```

となっている。

`Pid`も単なる`usize`ではなく、PIDを表す専用型として扱われている。

### processの確保

```rust
let (new_proc, new_inner) =
    try_log!(PROC_TABLE.alloc());
```

`PROC_TABLE.alloc()`は新しい`Proc`だけでなく、そのprocessの`SpinLockGuard`も返す。

したがって、

> 新しく確保したprocessのlockを現在保持している

という状態がguardという値として表現されている。

また、processの初期化に失敗した場合は、

```rust
new_proc.free(new_inner);
```

とする。

`free()`へguardを渡す必要があるため、

> processを解放するには、そのprocessのlockを保持している必要がある

という条件をAPIとして表現している。

### TrapFrame

親processのTrapFrameは、

```rust
new_trapframe.clone_from(trapframe);
```

によってコピーされる。

`clone_from()`は`Clone` traitのメソッドである。

処理そのものはCの、

```c
*(np->trapframe) = *(p->trapframe);
```

と大きくは変わらないため、Rust固有の特徴としては比較的弱い。

### open files

Rustでは、

```rust
[Option<File>; NOFILE]
```

によってopen fileを管理する。

```rust
for (i, file) in data.open_files.iter_mut().enumerate() {
    if let Some(file) = file.as_mut() {
        new_data.open_files[i] = Some(file.dup());
    }
}
```

`None`は未使用fd、`Some(File)`は有効なfdを表す。

コピー時には同じindexへfileを複製するため、親子processでfd番号は維持される。

CではNULL pointerの有無によって同様の状態を表現しているため、Rustでは`Option<File>`によって「存在する／存在しない」という状態を型として明示している点が異なる。

また、

```rust
new_data.name = data.name.clone();
new_data.cwd = data.cwd.dup();
```

という違いも存在する。

`String`である`name`は値そのものを複製するため`clone()`を使う。

一方`cwd`はinodeというOS内部resourceを共有するため、reference countを増加させる`dup()`を使用している。

### 親子関係とlock

Octoposでは親子関係を、

```rust
PROC_TABLE.parents
```

で管理している。

```rust
{
    let mut parents = PROC_TABLE.parents.lock();
    parents[new_proc.id] = Some(proc.id);
}
```

ここではlock guardの寿命をブロックによって限定している。

ブロックを抜けるとguardがdropされるため、自動的にunlockされる。

このようにcritical sectionの範囲をRustのスコープによって表現している。

### unsafe

新しくallocateされたprocessに対して、

```rust
// # Safety: new_proc is not yet runnable,
// so we are the only ones with access to it
let new_data = unsafe { new_proc.data_mut() };
```

として`ProcData`への可変参照を取得している。

新しいprocessはまだRunnableではなく、他CPUからアクセスされないため、呼び出し側が排他的アクセスを保証している。

これは、Rustの型システムだけでは証明できないOS内部の不変条件を`unsafe`境界として明示している例である。

## 6. xv6との比較

今回確認した範囲では、Octoposとxv6のアルゴリズム自体は大きく変わらない部分が多い。

一方、Rust版では次のような違いが確認できた。

- `Result`や専用のerror enumによるエラー表現
- `VA`、`PA`、`Pid`など意味ごとの型
- `Option<File>`による状態表現
- sliceを利用したsafe API
- borrow checkerによる競合する参照の検出
- lock guardとRAIIによるlock管理
- 所有権によるresourceの移動
- `unsafe`操作の明示と局所化

特に、xv6ではpointerや整数、NULLなどによって表現されている情報が、Octoposでは型として表現されている部分が多い。

一方で、Rustを使用したことで全てのresource管理が自動化されているわけではない。

PageTableやinodeのreference countなど、明示的な解放や管理が必要なresourceも存在する。

## 7. 考察

### Rustによって安全になった部分

Rustによって、次のような問題をコンパイル時またはAPI設計によって防ぎやすくなっている。

- 競合する可変参照
- 所有権を失った値へのアクセス
- lockのunlock忘れ
- `None`と有効なresourceの区別
- 仮想アドレスと物理アドレスなど意味の異なる値の混同
- raw pointerをsafe APIまで広く露出すること

特にborrow checkerについては、実際に`inode.clone()`を削除することでコンパイルエラーを発生させ、競合する借用が検出されることを確認できた。

### Rustでもプログラマ責任が残る部分

一方、Rustによって全てのOSバグが防げるわけではない。

今回確認した範囲でも、

- 同一lockの二重取得による自己デッドロック
- lockを必要以上に長く保持する問題
- `unsafe`内部でのポインタ操作
- PageTableなどの手動resource管理
- inodeの独自reference count管理

などはプログラマが正しく設計する必要がある。

特にデッドロックについてはsafe Rustのみでも発生することを実験によって確認した。

このことから、Rustはメモリ安全性やresource管理の一部を改善できる一方、並行処理やOS内部の論理的不変条件については、依然として設計者の責任が大きいと考えられる。

## 8. 結論

Octoposはxv6の処理やアルゴリズムを大きく踏襲しているため、OSとしての基本構造そのものはxv6と似ている。

しかし実装を見ると、

- 所有権・借用
- `Result` / `Option`
- newtypeによる型の区別
- RAIIとlock guard
- sliceを用いたsafe API
- `unsafe`境界の明示
- resourceの所有権移動

など、Rust固有の特徴を利用した設計が複数確認できた。

そのため、Octoposは単純にxv6のCコードをRustの構文へ置き換えただけではなく、Rustの型システムや所有権モデルをある程度活用したOS実装であると考えられる。

一方で、PageTableやinode reference countのように手動管理が残っているresourceや、safe Rustでも発生するデッドロックなども確認できた。

したがって、Octoposは「Rustによって防げる障害」と「Rustを使用していても残る障害」の両方を調査できるため、Rustで実装されたOSのレジリエンス性を検討する対象として利用できる可能性がある。

