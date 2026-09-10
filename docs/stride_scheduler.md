# Stride Scheduler

- `Stride Scheduler`は、各プロセスに`pass`というフィールドを持たせ、`Runnable`なプロセスのうち`pass`が最小のものを優先して実行するスケジューラである。
- 各プロセスには`tickets`が与えられ、ある定数を`tickets`で割った値を`stride`とする。
- プロセスがスケジュールされるたびに、そのプロセスの`pass`へ`stride`を加算する。
- `tickets`が大きいプロセスほど`stride`が小さくなるため、より高い頻度で選択され、`tickets`に応じたCPU配分を実現できる。
- 既存の`Round Robin`が基本的に各`Runnable`プロセスを順番に選択するのに対し、`Stride Scheduler`では`pass`の大小を利用して実行対象を決定する。

## 今回の目的

- `xv6`上でCによって実装したStride Schedulerと、`octopos`上でRustによって実装したStride Schedulerを比較する。
- 単なる文法上の違いではなく、スケジューラ実装で扱うプロセス、ロック、CPU状態などのメモリオブジェクトに対して、Rustの型システムやRAIIによってどこまで安全性を高められるのかを確認する。
- 一方で、並行実行時の論理的な競合やcontext switchをまたぐlock protocolなど、Rustコンパイラだけでは保証できない境界についても整理する。

## 1. Scheduler本体（C vs Rust）

### Schedulerの戻り値の型（`void` vs `!`）

C版ではschedulerの戻り値は`void`である。

RustではCの`void`に近い型としてunit型`()`が存在するが、`octopos`のschedulerでは以下のようにnever型`!`が使われている。

```rust
pub unsafe fn stride_scheduler() -> !
```

`!`は単に「値を返さない」ことではなく、この関数が呼び出し元へ正常にreturnしないことを型として表している。

schedulerは内部で無限loopを実行し続けるため、`!`によって「schedulerは戻らない」という性質を型として表現できる。

### 現在のCPUで実行しているProcの表現

C版ではCPUが現在実行しているprocessをpointerで保持し、processを実行していない場合は`0`（NULL）を代入する。

```c
c->proc = best_p;

/* processからschedulerへ戻った後 */

c->proc = 0;
```

Rust版では`cpu.proc`が`Option<&Proc>`として表現されており、processを実行するときは`Some(proc)`、scheduler自身を実行しているときは`None`として表現する。

```rust
cpu.proc.replace(proc);

/* processからschedulerへ戻った後 */

cpu.proc.take();
```

`replace(proc)`によって`cpu.proc`は`Some(proc)`になり、`take()`によって中身を取り出して`None`へ戻る。

このため、

```text
process実行中   : cpu.proc = Some(proc)
scheduler実行中 : cpu.proc = None
```

という状態を`Option`型として表現できる。

Cではnullableなpointerとして「有効なProcへのpointer」と「processなし」を同じpointer型で表すのに対し、Rustでは`Some`/`None`として状態を型に含めることができる。

ただし、`Some(proc)`に論理的に正しいprocessが格納されているかどうかまではRustコンパイラは保証しない。

## 2. 次に実行するProcの選択

### C版

C版では各processのlockを取得して`state`と`pass`を確認する。

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

現在の`best_p`より小さい`pass`を持つprocessを見つけた場合、それまでの`best_p`のlockを解放し、新しい`best_p`のlockを保持したまま次のprocessを探索する。

そのため、最終的に選択された`best_p`については、ループ終了後もlockを保持している。

### Rust版

Rust版では以下の`min_pass()`で対象processを探索する。

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

したがって、C版のように現在のbest processのlockを保持したまま次のprocessを調べるのではなく、各processの`pass`だけを取得し、そのprocessのlockは次のprocessを見る前に解放される。

`min_pass()`から返された時点では選択されたprocessのlockも解放済みであるため、scheduler側でもう一度lockを取得する。

```rust
let mut inner = proc.inner.lock();

if inner.state != ProcState::Runnable {
    continue;
}
```

`min_pass()`で情報を取得してから再度lockするまでの間に、別CPUによってprocessのstateが変更される可能性がある。そのため、再lock後に`Runnable`であることを再確認している。

この実装ではselection中に複数の`ProcInner` lockを同時に保持しない一方、`min_pass()`で取得した情報はprocess table全体のatomic snapshotではない。

なお、この差はRust言語によって必然的に生じるものではなく、Rust版で採用した実装設計上の違いである。Rustでもlock guardを保持し続ける設計自体は可能である。

## 3. CPUへのprocess割り当て

C版では選択されたprocess pointerを直接CPUへ設定する。

```c
c->proc = best_p;
```

Rust版では`Option<&Proc>`へ設定する。

```rust
cpu.proc.replace(proc);
```

この時点で、

```text
C    : c->proc = best_p
Rust : cpu.proc = Some(proc)
```

となる。

processからschedulerへ戻った後は、

```text
C    : c->proc = 0
Rust : cpu.proc.take() → None
```

として、現在このCPU上でprocessを実行していないことを表す。

Rustではcurrent processの有無を`Option`によって明示できるため、NULLそのものを直接扱う必要がない。

## 4. 選択後のprocess lockとcontext switch

### C版

最終的に選択された`best_p`についてはlockを保持したまま、`pass`更新、state変更、CPUへのprocess設定、context switchを行う。

```c
best_p->pass += best_p->stride;
best_p->time++;

best_p->state = RUNNING;
c->proc = best_p;

swtch(&c->context, &best_p->context);

before_pid = best_p->pid;
c->proc = 0;

release(&best_p->lock);
```

`release()`を明示的に呼び出す必要があり、適切な位置で`release()`を呼ばなければlockの解放忘れにつながる。

### Rust版

Rust版では選択されたprocessを再lockしてからcontext switchを行う。

```rust
let mut inner = proc.inner.lock();

if inner.state != ProcState::Runnable {
    continue;
}

inner.state = ProcState::Running;
cpu.proc.replace(proc);

unsafe {
    swtch(&mut cpu.context, &proc.data().context)
};

inner.pass += inner.stride;
inner.n_schedule += 1;

cpu.proc.take();
```

`inner`は`SpinLockGuard`であり、通常はscopeを抜けると`Drop`によってlockが解放される。

そのためC版のような`release(&p->lock)`の明示的な呼び出しを通常は必要としない。

ただし、早い段階でlockを解放する必要がある場合には、

```rust
drop(inner);
```

のように明示的にguardをDropすることもある。

## 5. `swtch()`をまたぐlock protocol

C版・Rust版ともに、選択されたprocessのlockを保持した状態で`swtch()`へ入る。

Rust版では`SpinLockGuard`である`inner`自体はscheduler stack上に残ったままcontext switchが行われる。

```rust
let mut inner = proc.inner.lock();

unsafe {
    swtch(&mut cpu.context, &proc.data().context)
};
```

`octopos`/`xv6`では、process側が実行中にこのproc lockを解放し、schedulerへ戻る前に再取得するという特殊なlock handoff protocolを利用している。

そのため、RustのRAIIによって最終的なlockの解放忘れを防ぐことはできるものの、

```text
schedulerがlockを保持してswtch
        ↓
process側でlockを解放
        ↓
processからschedulerへ戻る前に再取得
        ↓
schedulerへ復帰
```

というOS固有のprotocolそのものの正しさまではRustコンパイラは保証しない。

また、`swtch()`自体が`unsafe`であるため、context、register、lock状態などについて必要なinvariantをカーネル実装側で保証する必要がある。

## 6. `pass`更新タイミングの違い

C版ではcontext switch前に`pass`を更新している。

```c
best_p->pass += best_p->stride;
best_p->time++;

swtch(&c->context, &best_p->context);
```

一方、今回のRust版ではprocessからschedulerへ戻った後に更新している。

```rust
unsafe {
    swtch(&mut cpu.context, &proc.data().context)
};

inner.pass += inner.stride;
inner.n_schedule += 1;
```

これはRustの型システムや安全性による違いではなく、今回それぞれのStride Schedulerを実装した際の実装方針の違いである。

したがって、CとRustの言語上の安全性比較とは分けて扱う。

## 7. Rustが保証・支援できるもの

- `Option<&Proc>`によってcurrent processの「存在する／存在しない」を`Some`/`None`として型で表現できる。
- Cのnullable pointerを直接操作する必要がなく、`Option`を利用するコードではcurrent processが存在しない場合を明示的に扱わせることができる。
- `SpinLockGuard`のRAIIによって、通常のscopeを抜ける際の`release()`忘れを防ぎやすい。
- lock guardのlifetimeによって、lock取得中のデータへのアクセス範囲がコード上で明示される。
- `stride_scheduler() -> !`によって、schedulerが正常にはreturnしないことを型として表現できる。
- `checked_add()`などを利用することで、整数overflowを明示的に検出するAPIを利用できる。

## 8. Rustでも保証できないもの

- `cpu.proc`に論理的に正しいprocessが設定されているか。
- `min_pass()`で選択したprocessが、再lockする時点でも`Runnable`であるか。
- 複数lock間のlock orderingやdeadlockが発生しないこと。
- SMP環境でprocess table全体を見たときのatomicityやschedulerのfairness。
- pass値の正規化を複数CPUからatomicに観測できること。
- context switchをまたぐproc lockのhandoff protocolが正しく守られていること。
- `unsafe`な`swtch()`で要求されるcontextやregisterのinvariant。
- Stride Schedulingアルゴリズムそのものが論理的に正しいこと。

つまりRustによって、Cでは開発者が手動で管理していたnullable pointerやlock guardのlifetimeなど、一部の状態・resource管理を型システムやRAIIへ移すことができる。

一方で、schedulerにおけるprocess stateの変化、SMP上の競合、lock ordering、context switchをまたぐlock protocolなど、OS固有の論理的なinvariantについては依然として開発者側で保証する必要がある。

## Scheduler本体の比較から分かったこと

Scheduler本体を比較した結果、Rust化によってStride Schedulingというアルゴリズムそのものが安全になるわけではないことが分かった。

一方で、以下のようにCでは開発者が手動で管理していた一部の状態をRustの型システムやRAIIによって表現・管理できる。

```text
C                             Rust

Proc * / NULL                 Option<&Proc>
acquire()/release()           SpinLockGuard + Drop
戻らないことは実装上の性質    戻り値型 !
```

しかし、

```text
- process stateが並行して変化する
- staleなscheduler選択結果
- lock ordering
- deadlock
- SMP上でのatomicity
- context switchをまたぐlock protocol
```

などはRustコンパイラだけでは防げない。

したがって、Rustによってメモリ安全性やresource lifetime管理の一部は強化できる一方、OSカーネルに必要な並行処理やscheduler固有の論理的安全性は依然としてプログラマが設計・検証する必要がある。

## 2. Proc構造体

- Cではヘッダーファイル（`.h`）で定義されていて、Rustではソースファイル（`.rs`）で定義されている
- Cでは`Proc`構造体の中に全てのフィールドがあって、コメントでlockするべきフィールドとlockしないで良いフィールドを分けているが、Rustではlockすべき場所は`Spinlock`、lockする必要がない（シングルスレッドでの更新）では`UnsafeCell`でユーザ自身が責任を持つフィールドに分離している

### Processの初期化

- Rustの場合、`iterator`を回して、`unsafe`を使用して`kernelstack`のポインタを入れている
- また、カーネル起動時にのみ呼び出す前提で、`UnsafeCell`で可変参照として取得してそれぞれのspを入れている（`data_mut`メソッド経由で更新）
- Procのprivateなフィールドを触る場合、Rustでは`unsafe`を明示した上で可変参照をとって変更、Cではどのフィールドに対しても`lock`を取得して更新する

```
// initialize the proc table.
void
procinit(void)
{
  struct proc *p;

  initlock(&pid_lock, "nextpid");
  initlock(&wait_lock, "wait_lock");
  for(p = proc; p < &proc[NPROC]; p++) {
      initlock(&p->lock, "proc");
      p->state = UNUSED;
      p->kstack = KSTACK((int) (p - proc));
  }
}
```

```
/// Initializes the process table.
///
/// # Safety
/// Must be called only once during kernel initialization.
pub unsafe fn init() {
    for proc in PROC_TABLE.iter() {
        // # Safety: we are during initialization, so we are the only ones with access to the proc
        unsafe { proc.data_mut() }.kstack = VA::from(kstack(proc.id));
    }

    println!("proc init");
```

- メソッド自体にも参照だけか可変参照が必要かで分けられている
  - これらはコンパイル時に検知されるので、不必要な可変参照の取得を検知して実行前に確認することができる

```
    /// Returns a reference to the trapframe.
    pub fn trapframe(&self) -> &TrapFrame {
        self.trapframe.as_ref().unwrap()
    }

    /// Returns a mutable reference to the trapframe.
    pub fn trapframe_mut(&mut self) -> &mut TrapFrame {
        self.trapframe.as_mut().unwrap()
    }

    /// Returns a reference to the user page table.
    pub fn pagetable(&self) -> &Uvm {
        self.pagetable.as_ref().unwrap()
    }

    /// Returns a mutable reference to the user page table.
    pub fn pagetable_mut(&mut self) -> &mut Uvm {
        self.pagetable.as_mut().unwrap()
    }
```

- dataの参照だけなら`unsafe`ブロックでの呼び出しは必要ないが、可変参照なら`unsafe`ブロック内で呼び出す必要がある

```
    pub fn data(&self) -> &ProcData {
        unsafe { &*self.data.get() }
    }

    /// Returns a mutable reference to the process's data.
    ///
    /// # Safety
    /// The caller must ensure they have exclusive access to the `Proc`. This is true if either
    ///     1. it's the current proc (most cases) or
    ///     2. the proc's state hasn't been set to Runnable/Sleeping yet (fork, allocproc).
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn data_mut(&self) -> &mut ProcData {
        unsafe { &mut *self.data.get() }
    }
```

- Cell
  - Cell<T>の更新はTを取り出してTを置き換える
  - 可変参照は必要ない
- RefCell
  - RefCell<T>では可変参照を取得しないでも更新ができる
  - 複数からの可変参照はruntimeで検知する
  - `borrow_mut`メソッド経由での中身を更新する
