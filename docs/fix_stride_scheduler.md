# xv6とoctoposにおけるStride SchedulerとProc構造体の比較

## 1. Stride Scheduler

Stride Schedulerは、各processに`pass`、`tickets`、`stride`を持たせ、`Runnable`なprocessのうち`pass`が最小のものを優先して実行するschedulerである。

`stride`は一定値を`tickets`で割った値として求め、processがscheduleされるたびに`pass`へ`stride`を加算する。`tickets`が大きいprocessほど`stride`が小さくなるため、より高頻度に選択され、ticketsに応じたCPU時間の配分を実現できる。

今回、C版xv6上で実装したStride SchedulerとRust版octopos上で実装したStride Schedulerを比較し、単なる文法上の違いだけでなく、Rustの型システム、`Option`、RAII、`SpinLockGuard`、`unsafe`などによって、schedulerで扱う状態やresourceの安全性をどこまで高められるかを確認した。

### Schedulerの戻り値

C版ではschedulerは`void`を返す関数として実装される。

一方、Rust版では、

```rust
pub unsafe fn stride_scheduler() -> !
```

のようにnever型`!`を使用する。

`!`は単に値を返さないことを意味するのではなく、schedulerが無限loopを続け、呼び出し元へ正常にreturnしないことを型として表現している。

### 現在CPU上で実行しているprocessの表現

C版では現在実行しているprocessをpointerとして保持する。

```c
c->proc = best_p;
```

processを実行していない場合は、

```c
c->proc = 0;
```

としてNULLを格納する。

Rust版では`Option<&Proc>`を利用し、

```rust
cpu.proc.replace(proc);
```

で`Some(proc)`を格納し、

```rust
cpu.proc.take();
```

で`None`へ戻す。

したがって、

```text
process実行中   : Some(proc)
scheduler実行中 : None
```

という状態を型として明示できる。

Cではnullable pointerとしてprocessの有無を表現するのに対し、Rustでは`Option`によって「値が存在するかどうか」を型に含めることができる。ただし、`Some(proc)`に論理的に正しいprocessが格納されているかまではコンパイラは保証しない。

### 次に実行するprocessの選択

C版ではprocess tableを走査し、それぞれのprocessのlockを取得して`state`と`pass`を確認する。

最小の`pass`を持つprocessを見つけた場合、それまで候補だったprocessのlockを解放し、新しい候補のlockを保持したまま探索を続ける。そのため、最終的に選ばれたprocessのlockはloop終了後も保持される。

一方Rust版では、`min_pass()`で各processを順番にlockし、`state`と`pass`を確認する。

```rust
let inner = proc.inner.lock();
```

`pass`を取得した後は`SpinLockGuard`がscopeを抜けることで自動的にDropされ、lockが解放される。

そのため、Rust版ではprocess tableの探索中に複数processのlockを同時に保持しない。

ただし、`min_pass()`からprocessを選択してscheduler側で再びlockするまでの間に、他CPUによってprocess stateが変更される可能性がある。

そのため再lock後に、

```rust
if inner.state != ProcState::Runnable {
    continue;
}
```

として、再び`Runnable`かどうかを確認している。

この違いはRustという言語によって必然的に生じたものではなく、それぞれのschedulerの実装方針の違いである。

### Lock管理とRAII

C版では、

```c
acquire(&p->lock);
...
release(&p->lock);
```

のように、lock取得と解放を明示的に行う必要がある。

そのため`release()`の呼び出しを忘れたり、適切でない場所で解放したりする可能性がある。

Rust版では、

```rust
let mut inner = proc.inner.lock();
```

で取得した`SpinLockGuard`がscopeを抜けると、`Drop`によってlockが自動的に解放される。

したがってRAIIにより、通常のcontrol flowにおけるlock解放忘れを防ぎやすい。

必要に応じて、

```rust
drop(inner);
```

によって明示的に早くlockを解放することもできる。

### Context switchをまたぐlock protocol

C版・Rust版ともに、選択されたprocessのlockを保持した状態で`swtch()`を呼び出す。

Rust版でも`SpinLockGuard`がscheduler stack上に残ったままcontext switchが発生する。

xv6 / octoposでは、

```text
schedulerがproc lockを保持
        ↓
context switch
        ↓
process側でlockを解放
        ↓
schedulerへ戻る前に再取得
        ↓
schedulerへ復帰
```

という特殊なlock handoff protocolを利用する。

RustのRAIIによって最終的なlock解放忘れを防ぎやすくすることはできるが、このprotocolそのものが論理的に正しく守られているかはRustコンパイラでは保証できない。

また、`swtch()`自体が`unsafe`であるため、register、context、lock状態などのinvariantはkernel側で保証する必要がある。

### `pass`更新タイミング

C版ではcontext switch前に`pass`を更新する。

```c
best_p->pass += best_p->stride;
swtch(...);
```

今回のRust版ではcontext switchからschedulerへ戻った後に更新している。

```rust
swtch(...);

inner.pass += inner.stride;
inner.n_schedule += 1;
```

これはCとRustの言語仕様による違いではなく、Stride Schedulerを実装した際の方針の違いである。

### Rustが支援できる安全性

Rust版では、以下のような部分を型システムやRAIIによって表現・管理できる。

```text
C                             Rust

Proc * / NULL                 Option<&Proc>
acquire()/release()           SpinLockGuard + Drop
戻らないことは実装上の性質    never型 !
```

これにより、nullable pointerの直接操作やlock解放忘れなど、Cでは開発者が手動で管理する必要のある一部の状態・resource管理をRust側へ移すことができる。

一方で、以下のようなOS固有の論理的安全性まではRustコンパイラでは保証できない。

- `cpu.proc`に論理的に正しいprocessが設定されているか
- process stateが並行して変更されないか
- `min_pass()`で得たselection結果が再lock時点でも有効か
- 複数lock間のlock ordering
- deadlock
- SMP環境におけるschedulerのatomicityやfairness
- pass normalizationの正しさ
- context switchをまたぐlock handoff protocol
- `swtch()`が要求するregisterやcontextのinvariant
- Stride Schedulingアルゴリズムそのものの正しさ

したがって、Rust化によってStride Schedulingアルゴリズム自体が自動的に安全になるわけではない。

Rustは主にメモリ安全性やresource lifetime管理を支援するが、scheduler固有の並行処理やOSの状態遷移に関するinvariantについては、依然としてkernel開発者が設計・検証する必要がある。

---

## 2. Proc構造体

C版xv6とRust版octoposでは、processを表す構造体の設計にも違いがある。

C版では`struct proc`内にprocessに関するフィールドをまとめ、それぞれのフィールドをどのlockで保護するかなどの条件をコメントや実装上の規約によって管理する。

Rust版では、複数の実行主体からアクセスされ排他制御が必要なデータと、OS側の条件によってexclusive accessを保証するデータを分離し、`SpinLock`や`UnsafeCell`などを利用して表現している。

### Processの初期化

C版では`procinit()`でprocess tableを走査し、各processについてlock、state、kernel stackなどを初期化する。

Rust版では、

```rust
for proc in PROC_TABLE.iter() {
    unsafe { proc.data_mut() }.kstack = VA::from(kstack(proc.id));
}
```

のようにprocess tableをiteratorで走査し、`data_mut()`によって`ProcData`への可変参照を取得してkernel stackを設定する。

Rust版の`init()`はkernel initialization中に一度だけ呼ばれることを前提としている。

この時点では他の実行主体から`PROC_TABLE`へアクセスされないため、

```text
kernel initialization中
        ↓
他のCPU/processからアクセスされない
        ↓
exclusive accessが成立
        ↓
data_mut()を使用可能
```

という条件をkernel側が保証する。

### 共有参照と可変参照

Rust版では、参照だけを返すメソッドと可変参照を返すメソッドを明確に分けることができる。

例えば、

```rust
pub fn trapframe(&self) -> &TrapFrame
pub fn trapframe_mut(&mut self) -> &mut TrapFrame
```

や、

```rust
pub fn pagetable(&self) -> &Uvm
pub fn pagetable_mut(&mut self) -> &mut Uvm
```

のようにAPIを分離している。

Rustのborrow checkerは、複数の可変参照が同時に存在することや、可変参照と共有参照が競合することなどをコンパイル時に検査できる。

Cではpointerを通して直接構造体を操作できるため、このようなアクセス条件は主に開発者側の規約によって管理される。

### `UnsafeCell`による`ProcData`へのアクセス

Rust版では`ProcData`の内部可変性を実現するために`UnsafeCell`を利用している。

```rust
pub fn data(&self) -> &ProcData {
    unsafe { &*self.data.get() }
}
```

`UnsafeCell::get()`は内部データへのraw pointerを返し、それを共有参照へ変換している。

可変アクセスの場合は、

```rust
pub unsafe fn data_mut(&self) -> &mut ProcData {
    unsafe { &mut *self.data.get() }
}
```

として、`*mut ProcData`を`&mut ProcData`へ変換する。

このときexclusive accessが成立しているかどうかをRustコンパイラは判断できないため、`data_mut()`は`unsafe fn`となっている。

呼び出し側は、

- current processである
- まだ`Runnable`や`Sleeping`になっていない
- kernel initialization中である

など、OS側の条件によってexclusive accessが成立していることを保証する必要がある。

---

## 3. Interior Mutability

Rustでは、共有参照を持った状態でも内部の値を変更するInterior Mutabilityを実現するために、`Cell`、`RefCell`、`UnsafeCell`などが存在する。

### `Cell<T>`

`Cell<T>`では可変参照を取得する必要がなく、

```rust
get()
set()
replace()
```

などを使って値を取り出したり、丸ごと置き換えたりできる。

主に`Copy`可能な小さい値などに利用される。

### `RefCell<T>`

`RefCell<T>`も共有参照から内部データを変更できる。

```rust
borrow()
borrow_mut()
```

を利用して内部への参照を取得する。

通常のborrow checkerがコンパイル時に行う借用規則の検査を、`RefCell`ではruntimeに行う。

そのため、mutable borrowが複数存在した場合や、mutable borrowとimmutable borrowが競合した場合にはpanicする。

ただし、`RefCell`は複数CPUや複数thread間の排他制御を行う機構ではない。

### `UnsafeCell<T>`

`UnsafeCell<T>`はInterior Mutabilityを実現するための最も基本的な仕組みである。

`get()`によって得られるのは`&mut T`ではなく、

```rust
*mut T
```

というraw pointerである。

raw pointerから参照を作ったりdereferenceしたりする操作では、安全性を開発者側で保証する必要がある。

`Cell`や`RefCell`なども内部では`UnsafeCell`を利用して実装されている。

---

## 4. `SpinLock`と`UnsafeCell`の使い分け

`SpinLock`と`UnsafeCell`は目的が異なる。

`SpinLock`は、複数CPUや複数の実行主体から同じデータへアクセスする可能性がある場合に、同時アクセスを防ぐための排他制御として利用する。

一方、`UnsafeCell`自体には排他制御機能はない。

octoposでは、

```text
複数の実行主体からアクセスされる可能性がある
        ↓
SpinLockによって排他制御

OSの状態・実行規約によって
exclusive accessを保証できる
        ↓
UnsafeCell + unsafe
```

というように使い分けている。

Rustのborrow checkerは、

```text
current processか
process stateが何か
kernel initialization中か
特定のOS上のlock protocolが成立しているか
```

といったOS固有の意味までは自動的に理解できない。

そのため、Rustの型システムだけでは表現できない安全条件については`unsafe`を境界として明示し、その条件をkernel開発者が保証する。

---

## まとめ

C版xv6とRust版octoposを比較すると、Rust化によってOSの論理そのものが自動的に安全になるわけではない。

一方でRustでは、

```text
Option
& / &mut
SpinLockGuard
RAII
UnsafeCell
unsafe fn
never型 !
```

などを利用することで、Cではコメントや開発者の規約として管理していた一部の条件を型やAPIとして明示することができる。

特に、

```text
NULLの扱い
lockのlifetime
共有参照と可変参照
内部可変性
unsafeな操作の境界
```

についてはRustによる安全性向上が期待できる。

一方、

```text
process stateの正しさ
schedulerのselection race
SMP上の競合
deadlock
lock ordering
context switchをまたぐlock protocol
schedulerアルゴリズムそのものの正しさ
```

といったOS固有のinvariantはRustだけでは保証できない。

したがって、Rust kernelでは、Rustの型システムで保証できる部分を可能な限りsafeなAPIとして表現し、それだけでは保証できないOS固有の条件を`unsafe`の境界として明示し、kernel側で管理することが重要である。