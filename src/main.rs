use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::Arc;

struct MCSLock<T> {
    last: AtomicPtr<MCSNode<T>>, // キューの最後尾
    data: UnsafeCell<T>,         // 保護対象データ
}

struct MCSNode<T> {
    next: AtomicPtr<MCSNode<T>>,
    locked: AtomicBool,
    mcs_lock: Arc<MCSLock<T>>,
}

impl<T> MCSLock<T> {
    fn new(v: T) -> MCSLock<T> {
        MCSLock {
            last: AtomicPtr::new(null_mut()),
            data: UnsafeCell::new(v),
        }
    }

    fn get_locker(self: Arc<MCSLock<T>>) -> MCSNode<T> {
        MCSNode {
            next: AtomicPtr::new(null_mut()),
            locked: AtomicBool::new(false),
            mcs_lock: self.clone(),
        }
    }
}

unsafe impl<T> Sync for MCSLock<T> {}
unsafe impl<T> Send for MCSLock<T> {}

impl<T> MCSNode<T> {
    fn lock(&mut self) -> MCSLockGuard<T> {
        // 自身をキューの最後尾とする
        let ptr = self as *mut Self;
        let prev = self.mcs_lock.last.swap(ptr, Ordering::SeqCst);

        // 最後尾がnullの場合は誰もロックを獲得しようとしていないためロック獲得
        // null以外の場合は、自身をキューの最後尾に追加
        if prev != null_mut() {
            // ロック獲得中と設定
            self.locked.store(true, Ordering::SeqCst);

            // 自身をキューの最後尾に追加
            let prev = unsafe { &*prev };
            prev.next.store(ptr, Ordering::SeqCst);

            // 他のスレッドからfalseに設定されるまでスピン
            while self.locked.load(Ordering::SeqCst) {}
        }

        MCSLockGuard { node: self }
    }
}

struct MCSLockGuard<'a, T> {
    node: &'a mut MCSNode<T>,
}

impl<'a, T> Drop for MCSLockGuard<'a, T> {
    fn drop(&mut self) {
        // 自身の次のノードがnullかつ自身が最後尾のノードなら、最後尾をnullに設定
        if self.node.next.load(Ordering::SeqCst) == null_mut() {
            let ptr = self.node as *mut MCSNode<T>;
            if let Ok(_) = self.node.mcs_lock.last.compare_exchange(
                ptr,
                null_mut(),
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                return;
            }
        }

        // 自身の次のスレッドがlock関数実行中なので、その終了を待機
        while self.node.next.load(Ordering::SeqCst) == null_mut() {}

        // 自身の次のスレッドを実行可能に設定
        let next = unsafe { &mut *self.node.next.load(Ordering::SeqCst) };
        next.locked.store(false, Ordering::SeqCst);

        // ノードを初期化
        self.node.next.store(null_mut(), Ordering::SeqCst);
    }
}

// 保護対象データのimmutableな参照はずし (10)
impl<'a, T> Deref for MCSLockGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.node.mcs_lock.data.get() }
    }
}

// 保護対象データのmutableな参照はずし (11)
impl<'a, T> DerefMut for MCSLockGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.node.mcs_lock.data.get() }
    }
}

const NUM_THREADS: usize = 4;
const NUM_LOOP: usize = 1000000;

fn main() {
    let lock = Arc::new(MCSLock::new(0));
    let mut v = Vec::new();

    for _ in 0..NUM_THREADS {
        let lock0 = lock.clone();
        // スレッド生成
        let t = std::thread::spawn(move || {
            let mut locker = lock0.get_locker();
            for _ in 0..NUM_LOOP {
                // ロック
                let mut data = locker.lock();
                *data += 1;
            }
        });
        v.push(t);
    }

    for t in v {
        t.join().unwrap();
    }

    let mut locker = lock.get_locker();
    println!(
        "COUNT = {} (expected = {})",
        *locker.lock(),
        NUM_LOOP * NUM_THREADS
    );
}
