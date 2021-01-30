use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::ptr::null_mut;
use std::sync::atomic::{fence, AtomicBool, AtomicPtr, Ordering};
use std::sync::Arc;

struct MCSLock<T> {
    last: AtomicPtr<MCSNode<T>>, // キューの最後尾
    data: UnsafeCell<T>,         // 保護対象データ
}

struct MCSNode<T> {
    next: AtomicPtr<MCSNode<T>>,
    locked: AtomicBool,
}

impl<T> MCSLock<T> {
    fn new(v: T) -> MCSLock<T> {
        MCSLock {
            last: AtomicPtr::new(null_mut()),
            data: UnsafeCell::new(v),
        }
    }

    fn lock(&self) -> MCSLockGuard<T> {
        // 自身をキューの最後尾とする
        let mut guard = MCSLockGuard {
            node: MCSNode {
                next: AtomicPtr::new(null_mut()),
                locked: AtomicBool::new(false),
            },
            mcs_lock: self,
        };

        let ptr = &mut guard.node as *mut MCSNode<T>;
        let prev = self.last.swap(ptr, Ordering::Relaxed);

        // 最後尾がnullの場合は誰もロックを獲得しようとしていないためロック獲得
        // null以外の場合は、自身をキューの最後尾に追加
        if prev != null_mut() {
            // ロック獲得中と設定
            guard.node.locked.store(true, Ordering::Relaxed);

            // 自身をキューの最後尾に追加
            let prev = unsafe { &*prev };
            prev.next.store(ptr, Ordering::Relaxed);

            // 他のスレッドからfalseに設定されるまでスピン
            while guard.node.locked.load(Ordering::Relaxed) {}
        }

        fence(Ordering::Acquire);
        guard
    }
}

unsafe impl<T> Sync for MCSLock<T> {}
unsafe impl<T> Send for MCSLock<T> {}

struct MCSLockGuard<'a, T> {
    node: MCSNode<T>,
    mcs_lock: &'a MCSLock<T>,
}

impl<'a, T> Drop for MCSLockGuard<'a, T> {
    fn drop(&mut self) {
        // 自身の次のノードがnullかつ自身が最後尾のノードなら、最後尾をnullに設定
        if self.node.next.load(Ordering::Relaxed) == null_mut() {
            let ptr = &mut self.node as *mut MCSNode<T>;
            if let Ok(_) = self.mcs_lock.last.compare_exchange(
                ptr,
                null_mut(),
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                return;
            }
        }

        // 自身の次のスレッドがlock関数実行中なので、その終了を待機
        while self.node.next.load(Ordering::Relaxed) == null_mut() {}

        // 自身の次のスレッドを実行可能に設定
        let next = unsafe { &mut *self.node.next.load(Ordering::Relaxed) };
        next.locked.store(false, Ordering::Release);
    }
}

// 保護対象データのimmutableな参照はずし
impl<'a, T> Deref for MCSLockGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mcs_lock.data.get() }
    }
}

// 保護対象データのmutableな参照はずし
impl<'a, T> DerefMut for MCSLockGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.mcs_lock.data.get() }
    }
}

const NUM_THREADS: usize = 4;
const NUM_LOOP: usize = 200000000;

fn main() {
    let lock = Arc::new(MCSLock::new(0));
    let mut v = Vec::new();

    for _ in 0..NUM_THREADS {
        let lock0 = lock.clone();
        // スレッド生成
        let t = std::thread::spawn(move || {
            for _ in 0..NUM_LOOP {
                // ロック
                let mut data = lock0.lock();
                *data += 1;
            }
        });
        v.push(t);
    }

    for t in v {
        t.join().unwrap();
    }

    println!(
        "COUNT = {} (expected = {})",
        *lock.lock(),
        NUM_LOOP * NUM_THREADS
    );
}
