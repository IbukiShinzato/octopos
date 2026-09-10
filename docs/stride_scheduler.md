# Stride Scheduler実装を通したxv6とoctoposの比較

## 1. 目的

今回のタスクでは、Cで実装されたxv6とRustで実装されたoctoposの両方にStride Schedulerを実装し、その実装を通して両OSにどのような違いが現れるかを整理した。

Stride Schedulerそのものは、各processに`tickets`、`stride`、`pass`を持たせ、`Runnable`なprocessのうち`pass`が最小のprocessを次に実行するschedulerである。

`stride`は一定値を`tickets`で割った値として求め、processがscheduleされるたびに`pass`へ`stride`を加算する。`tickets`が大きいprocessほど`stride`が小さくなるため、より高い頻度で選択され、ticketsに応じたCPU時間配分を実現できる。

比較では、単なるCとRustの文法差ではなく、Stride Schedulerを実装・デバッグする過程で確認できた以下の点を中心に整理する。

1. Scheduler本体の実装
2. Kernel stack overflowの挙動
3. `Proc`構造体とprocess状態の管理
4. Syscallの実装
5. Rustによって安全性を高められる部分と、OS側で依然として保証する必要がある部分

---

## 2. Stride Scheduler本体の比較

### 2.1 Schedulerの戻り値

C版xv6ではschedulerは`void`を返す関数として実装される。

一方、octoposでは以下のようにnever型`!`が利用されている。

```rust
pub unsafe fn stride_scheduler() -> !
```

`!`は単に「値を返さない」ことではなく、この関数が呼び出し元へ正常にreturnしないことを型として表す。

schedulerは内部で無限loopを実行し続けるため、octoposでは「schedulerは戻らない」という性質を型に含めて表現できる。

### 2.2 現在CPUで実行しているprocessの表現

xv6ではCPUが現在実行しているprocessをpointerで保持し、processを実行していない場合はNULLを格納する。

```c
c->proc = best_p;

/* processからschedulerへ戻った後 */

c->proc = 0;
```

octoposでは`cpu.proc`が`Option<&Proc>`として表現される。

```rust
cpu.proc.replace(proc);

/* processからschedulerへ戻った後 */

cpu.proc.take();
```

このため、

```text
process実行中   : Some(proc)
scheduler実行中 : None
```

という状態を`Option`型として表現できる。

xv6ではnullable pointerとして「processが存在する／存在しない」を表現するのに対し、octoposでは`Some`/`None`として状態を型に含められる。

ただし、`Some(proc)`に論理的に正しいprocessが格納されているかどうかまではRustコンパイラは保証しない。

### 2.3 次に実行するprocessの選択

xv6版ではprocess tableを走査し、各processのlockを取得して`state`と`pass`を確認する。

```c
for(p = proc; p < &proc[NPROC]; p++) {
  acquire(&p->lock);

  if(p->state == RUNNABLE) {
    if (best_p == 0 || p->pass < best_p->pass) {
      if (best_p) {
        release(&best_p->lock);
      }

      best_p = p;
      continue;
    }
  }

  release(&p->lock);
}
```

現在の`best_p`より小さい`pass`を持つprocessを見つけた場合、それまでの候補のlockを解放し、新しい候補のlockを保持したまま探索を続ける。

そのため、最終的に選択された`best_p`についてはloop終了後もlockを保持している。

一方、octoposでは`min_pass()`で各processを順番にlockして`state`と`pass`を確認する。

```rust
pub fn min_pass(&self, states: &[ProcState]) -> Option<(&Proc, usize)> {
    self.iter()
        .filter_map(|proc| {
            let inner = proc.inner.lock();

            if states.contains(&inner.state) {
                Some((proc, inner.pass))
            } else {
                None
            }
        })
        .min_by_key(|&(_proc, pass)| pass)
}
```

各processについて、

```text
lock取得
  ↓
state確認
  ↓
passをコピー
  ↓
closure終了
  ↓
SpinLockGuardがDropされてunlock
```

という流れになる。

したがってoctoposでは、process tableの探索中に複数の`ProcInner` lockを同時に保持しない。

ただし、`min_pass()`でprocessを選択してからscheduler側で再度lockするまでの間に、別CPUによってprocessのstateが変更される可能性がある。

そのため再lock後に、

```rust
let mut inner = proc.inner.lock();

if inner.state != ProcState::Runnable {
    continue;
}
```

として、processが依然として`Runnable`であることを確認する。

この差はRustによって必然的に生じるものではなく、xv6版とoctopos版で採用したscheduler実装方針の違いである。

### 2.4 Lock管理

xv6ではlockの取得・解放を明示的に行う。

```c
acquire(&p->lock);
...
release(&p->lock);
```

そのため、開発者が適切な位置で`release()`を呼び出す必要がある。

octoposでは、

```rust
let mut inner = proc.inner.lock();
```

によって`SpinLockGuard`を取得し、通常はscopeを抜けた際の`Drop`によってlockが自動的に解放される。

必要な場合には、

```rust
drop(inner);
```

として明示的に早くlockを解放することもできる。

この点では、RustのRAIIによって通常のcontrol flowにおけるlock解放忘れを防ぎやすくなっている。

### 2.5 Context switchをまたぐlock protocol

xv6とoctoposの両方で、選択されたprocessのlockを保持した状態で`swtch()`へ入る。

octoposでも`SpinLockGuard`である`inner`はscheduler stack上に残ったままcontext switchされる。

概念的には、

```text
schedulerがproc lockを保持
        ↓
context switch
        ↓
process側でlockを解放
        ↓
processからschedulerへ戻る前に再取得
        ↓
schedulerへ復帰
```

という特殊なlock handoff protocolになる。

RAIIによって最終的なlock解放を管理しやすくすることはできるが、このcontext switchをまたぐprotocol自体が正しいことをRustコンパイラが証明してくれるわけではない。

また、`swtch()`自体が`unsafe`であるため、context、register、stack、lock状態などについて必要なinvariantをkernel側で保証する必要がある。

### 2.6 `pass`更新タイミング

今回実装したxv6版ではcontext switch前に`pass`を更新する。

```c
best_p->pass += best_p->stride;
best_p->time++;

swtch(&c->context, &best_p->context);
```

一方、octopos版ではprocessからschedulerへ戻った後に更新する。

```rust
unsafe {
    swtch(&mut cpu.context, &proc.data().context)
};

inner.pass += inner.stride;
inner.n_schedule += 1;
```

この差もRustとCの言語仕様によるものではなく、今回それぞれにStride Schedulerを実装した際の実装方針の違いである。

---

## 3. Kernel stack overflowの挙動

Stride Scheduler実装後のデバッグではkernel stackについても確認した。

octoposではrelease build時に、

```rust
pub const PGSHIFT: usize = 12;
pub const PGSIZE: usize = 1 << PGSHIFT; // 4 KiB

#[cfg(not(debug_assertions))]
pub const NKSTACK_PAGES: usize = 1;
```

となっており、1 processあたりのkernel stackは1 page、すなわち4 KiBである。

xv6でも各processにkernel stackを割り当て、その隣にguard pageを設ける構成を取る。

kernel stackが範囲を越えてguard pageなどのunmapped領域へアクセスするとpage faultとなり、kernel trapとして検出される。

この点で重要なのは、Rustでkernelが実装されていてもkernel stack overflowそのものをborrow checkerが防止するわけではないことである。

Rustの型システムが主に保証するのは参照や所有権に関するmemory safetyであり、

```text
再帰が深すぎる
stack frameが大きすぎる
kernel stackの4 KiBを使い切る
```

といったstack使用量そのものを静的に保証するものではない。

したがってkernel stack overflowに関しては、xv6とoctoposの両方で、stack配置・guard page・trap処理などkernel側のmechanismによって異常を検出する必要がある。

今回のデバッグではkernel stackの詳細なfault原因の追跡までは行わず、release buildでのstack sizeと仮想メモリ上の配置、overflow時にはpage faultとして現れる点までを確認した。

---

## 4. `Proc`構造体の比較

Stride Schedulerを実装する際には、processの`state`、`pass`、`stride`、`tickets`などを扱うため、xv6とoctoposの`Proc`構造体の違いが直接実装方法に影響した。

### 4.1 構造体とlockの設計

xv6では`struct proc`にprocessに関するfieldをまとめ、それぞれのfieldをどのlockで保護するかなどの条件をコメントや実装規約として管理する。

octoposでは、複数の実行主体からアクセスされ排他制御が必要なデータと、OS側の条件によってexclusive accessを保証するデータを分離し、`SpinLock`や`UnsafeCell`を利用している。

概念的には、

```text
複数の実行主体から同時アクセスされる可能性がある
        ↓
SpinLockによって排他制御

OSの状態・実行規約によってexclusive accessを保証できる
        ↓
UnsafeCell + unsafe
```

という使い分けになる。

`UnsafeCell`自体には排他制御機能はないため、同時アクセスしないことをOS側のinvariantとして保証する必要がある。

### 4.2 Processの初期化

xv6では`procinit()`でprocess tableを走査し、lock、state、kernel stackなどを初期化する。

```c
void
procinit(void)
{
  struct proc *p;

  initlock(&pid_lock, "nextpid");
  initlock(&wait_lock, "wait_lock");

  for(p = proc; p < &proc[NPROC]; p++) {
      initlock(&p->lock, "proc");
      p->state = UNUSED;
      p->kstack = KSTACK((int)(p - proc));
  }
}
```

octoposでは、

```rust
pub unsafe fn init() {
    for proc in PROC_TABLE.iter() {
        unsafe { proc.data_mut() }.kstack = VA::from(kstack(proc.id));
    }

    println!("proc init");
}
```

のようにprocess tableをiteratorで走査し、`data_mut()`を通して`ProcData`への可変参照を取得する。

`init()`はkernel initialization中に一度だけ呼ばれることを前提としている。

この時点では他の実行主体から`PROC_TABLE`へアクセスされないため、

```text
kernel initialization中
        ↓
他のCPU/processからアクセスされない
        ↓
exclusive accessが成立
        ↓
data_mut()を利用できる
```

という条件をkernel側が保証する。

### 4.3 共有参照と可変参照

octoposでは参照だけを返すメソッドと、可変参照を返すメソッドを分けている。

```rust
pub fn trapframe(&self) -> &TrapFrame
pub fn trapframe_mut(&mut self) -> &mut TrapFrame

pub fn pagetable(&self) -> &Uvm
pub fn pagetable_mut(&mut self) -> &mut Uvm
```

Rustのborrow checkerは、複数の可変参照の同時存在や、可変参照と共有参照の競合などをコンパイル時に検査できる。

xv6ではpointerを通してfieldを直接操作できるため、アクセス可能な条件については開発者側の規約への依存が大きい。

### 4.4 `UnsafeCell`による`ProcData`へのアクセス

octoposでは`ProcData`の内部可変性を実現するために`UnsafeCell`を利用している。

```rust
pub fn data(&self) -> &ProcData {
    unsafe { &*self.data.get() }
}

pub unsafe fn data_mut(&self) -> &mut ProcData {
    unsafe { &mut *self.data.get() }
}
```

`UnsafeCell::get()`が返すのは`&mut T`ではなく`*mut T`というraw pointerである。

`data_mut()`ではこのraw pointerから`&mut ProcData`を作るため、その時点でexclusive accessが成立していることを呼び出し側が保証する必要がある。

このように、xv6ではコメントや実装規約として存在する一部のアクセス条件を、octoposではsafe APIと`unsafe` APIの境界として表現している。

---

## 5. Syscallの比較

Stride Schedulerそのものの実装箇所ではないが、process状態やuser/kernel境界の扱い方を比較するため、syscall経路も確認した。

### 5.1 syscall入口

xv6では`usys.S`で`a7`レジスタにシステムコール番号を設定し、`ecall`を実行する。

octoposでは`inline asm`を利用してRISC-V命令を記述し、引数を`a0`以降のregisterへ、システムコール番号を`a7`へ設定した上で`ecall`を実行する。

user modeで`ecall`が発生するとtrapし、trampolineを経由してkernel側のtrap handlerへ移行し、trapframeに保存されたregister値を利用してsyscall処理へ進む。

### 5.2 syscall番号とdispatch

xv6ではtrapframeの`a7`からシステムコール番号を取得し、その値をindexとして対応する関数を呼び出す。

octoposでは`a7`の値を`Syscall`型へ変換し、`match`によって対応するsyscallへdispatchする。

また、octoposではsyscall引数を`Args`としてまとめ、各syscallへ参照として渡す設計になっている。

このため、xv6が整数値を中心にdispatchするのに対して、octoposではsyscall番号自体を型として表現できる。

### 5.3 引数取得

xv6では`argint`、`argaddr`など、取得する引数に応じた関数を利用する。

octoposでは`SyscallArgs`のメソッドとして引数取得処理を実装し、仮想アドレスには単なる整数ではなく`VA`型を利用する。

そのため、

```text
xv6     : uint64などの整数値
octopos : VA
```

のように、仮想アドレスであることを型として区別できる。

### 5.4 user memoryアクセス

xv6とoctoposの両方で、user virtual addressを扱う場合にはpage tableを利用してuser memoryへアクセスする。

xv6では`pagetable`を`copyin`や`copyout`などの関数へ渡す設計が中心である。

octoposではpage tableを表す型のメソッドとしてuser memory操作を実装している箇所があり、失敗は`Result`として表現される。

ただし、userから渡されたvirtual addressが本当に有効な領域を指しているかどうかをRustの型だけで保証することはできない。

そのためkernel側でpage tableやaddress rangeを確認し、不正な場合にはerrorとして処理する必要がある。

### 5.5 戻り値とerror処理

xv6ではsyscall失敗時に`-1`を返す設計が多い。

一方、octoposでは`Result`と`SysError`を利用する。

```rust
pub enum SysError {
    NotPermitted,
    NoEntry,
    NoProcess,
    Interrupted,
    IoError,
    /* … */
}
```

このため、

```text
成功
失敗
失敗した理由
```

を型として分離して扱える。

### 5.6 `unsafe`の境界

octoposのsyscall本体の多くはsafe Rustとして記述できるが、Rustの型システムだけでは安全性を証明できない操作では`unsafe`が必要になる。

例として、

- raw pointerのdereference
- user virtual addressからsliceを生成する処理
- `UnsafeCell`を通した可変アクセス
- inline assembly
- exclusive accessをOS側の状態によって保証する処理

などがある。

重要なのは、userから不正なaddressが渡された場合でも、それを安全に検証・拒否する責任はkernel側にあることである。

### 5.7 `read` / `write`

xv6とoctoposの`read` / `write`は、処理の大枠は共通している。

```text
file descriptor
    ↓
Fileを取得
    ↓
inode / pipe / deviceなどへ処理を委譲
    ↓
user memoryとのデータ転送
```

一方、表現方法には違いがある。

| 項目 | xv6 | octopos |
|---|---|---|
| user address | `uint64`など | `VA` |
| file | `struct file *` | `File`構造体 |
| read/write | `fileread()` / `filewrite()` | `File`のメソッド |
| error | 主に`-1` | `Result` / `SysError` |
| lock管理 | 明示的なlock操作 | `SpinLockGuard`など |
| user memory | 整数addressを検査してcopy | 型を利用しつつ、必要な境界は`unsafe` |

ただし、octoposでもuser addressの妥当性や、inode・pipeなど複数resource間のlock orderingまで型だけで保証できるわけではない。

---

## 6. Stride Scheduler実装を通して見えたxv6とoctoposの違い

今回の比較で最も重要だったのは、Stride Schedulingアルゴリズムそのものはxv6でもoctoposでも大きく変わらない一方、そのアルゴリズムをkernel内部で安全に実装・管理する方法が異なるという点である。

### Rustによって型やRAIIに移せる部分

```text
xv6                           octopos

Proc * / NULL                 Option<&Proc>
整数としてのaddress           VA
-1によるerror                 Result / SysError
acquire()/release()           SpinLockGuard + Drop
共有・可変accessの規約         &T / &mut T
危険な操作がコード全体に存在   unsafe境界を明示
戻らないことは実装上の性質     never型 !
```

octoposでは、xv6でpointer、整数値、NULL、手動lock管理、コメント上の規約として表現されている一部の条件をRustの型システムやRAIIへ移すことができる。

このため、以下のような問題は防止・発見しやすくなる。

- nullable pointerの扱い
- 通常のcontrol flowにおけるlock解放忘れ
- 共有参照と可変参照の競合
- addressやerrorの種類の取り違え
- unsafeな操作が存在する場所の把握

### Rustだけでは保証できない部分

一方、以下のようなOS固有の論理的安全性はRustだけでは保証できない。

- `cpu.proc`に論理的に正しいprocessが設定されているか
- process stateの遷移が正しいか
- `min_pass()`で得たselection結果が再lock時にも有効か
- schedulerのfairness
- pass値のoverflowやnormalizationの設計
- SMP環境でのatomicity
- lock ordering
- deadlock
- context switchをまたぐlock handoff protocol
- `swtch()`が要求するregister、context、stackのinvariant
- kernel stack overflow
- user pointerが意味的に正しい領域を指しているか
- Stride Schedulingアルゴリズムそのものが正しく実装されているか

つまりRustはOSの論理を自動的に正しくするものではない。

---

## 7. 全体レビュー

Stride Schedulerをxv6とoctoposの両方に実装した結果、schedulerアルゴリズムの中心部分はほぼ同じ考え方で実装できた。

どちらも、

```text
Runnableなprocessを探索
        ↓
passが最小のprocessを選択
        ↓
Runningへ変更
        ↓
context switch
        ↓
passを更新
```

という基本構造を持つ。

一方で、実装を支えるkernel内部の表現には大きな違いがあった。

xv6ではpointer、NULL、整数値、明示的な`acquire()` / `release()`、コメントによるlock規約などを利用し、開発者が多くの条件を手動で管理する。

octoposでは、`Option`、`Result`、`VA`、`SpinLockGuard`、`&T` / `&mut T`、`UnsafeCell`、`unsafe`などを利用し、xv6では暗黙的な規約だった条件の一部を型やAPIとして明示している。

特にStride Schedulerの実装では、processを選択する際のlock、current processの表現、`Proc`内部へのaccess、context switchなどを通して、この違いを確認できた。

ただし、Rustを利用しても、

```text
schedulerのselection race
process stateの論理的な正しさ
SMP上の競合
deadlock
lock ordering
context switch protocol
kernel stack overflow
```

などのOS固有の問題が自動的に解決されるわけではない。

したがって今回の実装を通して、

> **RustはStride Schedulerそのものを自動的に安全にするのではなく、schedulerを構成するpointer、lock、参照、error、resource lifetimeなどの一部を型システムやRAIIによって安全に管理しやすくする。一方、schedulerやkernel固有の並行処理・状態遷移・context switchのinvariantについては、依然としてOS開発者が設計・検証する必要がある。**

という違いがxv6とoctoposの間で確認できた。
