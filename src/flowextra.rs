use anyhow::Result;
use std::future::Future;

#[inline(always)]
pub async fn async_err_stop<R, T: Future<Output = Result<R>>, F: FnOnce() -> T>(f: F) -> Result<R> {
    f().await
}

#[macro_export]
macro_rules! aes {
    ($b:expr) => {
        $crate::flowextra::async_err_stop(|| async { $b })
    };
}

#[macro_export]
macro_rules! bb {
    ($e:expr) => {
        loop {
            $e;
            break;
        }
    };
}
