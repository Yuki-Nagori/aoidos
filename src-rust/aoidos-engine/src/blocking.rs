//! 阻塞存储边界共用线程池与脱敏中断错误；临界区内不 await。

use crate::fault::Fault;
/// # Errors
/// 原领域错误原样返回；任务 panic / 中断统一为 store.io，不暴露线程诊断。
pub async fn run<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, Fault> + Send + 'static,
) -> Result<T, Fault> {
    tokio::task::spawn_blocking(job)
        .await
        .map_err(interrupted)?
}
fn interrupted(_: tokio::task::JoinError) -> Fault {
    Fault::new("store.io", "存储任务中断")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn success_domain_failure_and_panicked_workers_are_distinct() {
        assert_eq!(run(|| Ok(7)).await.unwrap(), 7);
        assert_eq!(
            run::<()>(|| Err(Fault::bad_request()))
                .await
                .unwrap_err()
                .code,
            "app.bad-request"
        );
        assert_eq!(
            run::<()>(|| panic!("synthetic interruption"))
                .await
                .unwrap_err()
                .code,
            "store.io"
        );
    }
}
