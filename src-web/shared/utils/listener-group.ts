/** 逐项持有异步注册资源；失败或关闭后，迟到的监听也立即释放。 */
export function createListenerGroup() {
  let closed = false;
  const releases = new Set<() => void>();
  function close() {
    closed = true;
    releases.forEach((release) => release());
    releases.clear();
  }
  async function register(create: () => Promise<() => void>): Promise<void> {
    try {
      const release = await create();
      if (closed) release();
      else {
        // 每次注册独立拥有句柄，即使端口返回同一函数，也各释放一次。
        const off = () => {
          releases.delete(off);
          release();
        };
        releases.add(off);
      }
    } catch (error) {
      close();
      throw error;
    }
  }
  return { register, close };
}
