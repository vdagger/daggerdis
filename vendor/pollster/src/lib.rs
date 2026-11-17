//! Минимальная замена крейта pollster.
//!
//! Крейт нужен только `eframe` с feature `wgpu` — он там используется как
//! `pollster::block_on` для инициализации wgpu. На сборочной машине реестр
//! cargo принадлежит другому пользователю и недоступен на запись, поэтому
//! вместо скачивания pollster.crate подключается этот локальный крейт
//! (см. `[patch.crates-io]` в корневом Cargo.toml). Поведение то же:
//! блокируем текущий поток, пока future не завершится.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Выполняет future до конца, блокируя текущий поток.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match Pin::as_mut(&mut future).poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}
