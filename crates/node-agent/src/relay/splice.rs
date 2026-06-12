//! Linux splice(2) 零拷贝 TCP 转发(P6.2)。
//!
//! splice 在内核内部把数据从一个 fd 移动到另一个 fd,不经用户态——消除了大缓冲
//! pump 路径里 read→user buffer→write 的两次 memcpy。2Gbps 下这两次 memcpy 是
//! 1 核机型 CPU 的大头,splice 把它降到接近零。
//!
//! socket↔socket 不能直接 splice(内核要求至少一端是 pipe),所以每个方向借一个
//! 内核 pipe 中转:splice(src_socket → pipe) 填充,splice(pipe → dst_socket) 排空。
//!
//! 仅用于不限速路径:限速需要在用户态按令牌桶计量,数据必须过用户态,无法 splice。
//! 本模块仅在 `target_os = "linux"` 编译;其它平台 bridge 回退到 `pump`。
//!
//! 自研实现(参考过 zhboner/realm-io 的机制,但代码与计数策略均为本项目独立编写)。

use anyhow::Result;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::io::Interest;
use tokio::net::TcpStream;

use crate::stats::RuleCounter;

/// 期望的 pipe 容量。与不限速 pump 的 256KB 缓冲对齐;内核会向上取整到页边界,
/// 非特权进程超过 /proc/sys/fs/pipe-max-size(默认 1MB)会失败,届时回退内核默认 64KB。
const PIPE_CAP: i32 = 256 * 1024;

/// RAII 包裹一对 pipe fd(OwnedFd 在 Drop 时关闭,杜绝泄漏/double-close)。
struct Pipe {
    r: OwnedFd,
    w: OwnedFd,
    /// pipe 的实际容量(可能与 PIPE_CAP 不同),用于 pending 上界判断防忙循环。
    cap: usize,
}

fn make_pipe() -> io::Result<Pipe> {
    let mut fds = [0 as libc::c_int; 2];
    // O_NONBLOCK:splice 配合 try_io 的就绪模型需要非阻塞;O_CLOEXEC:exec 时自动关。
    let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY:pipe2 成功返回两个全新拥有的 fd,交给 OwnedFd 接管所有权。
    let r = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let w = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    // 设容量:F_SETPIPE_SZ 成功返回实际(取整后)大小;失败不致命,读回内核默认。
    let cap = unsafe {
        let set = libc::fcntl(w.as_raw_fd(), libc::F_SETPIPE_SZ, PIPE_CAP);
        if set > 0 {
            set as usize
        } else {
            let got = libc::fcntl(w.as_raw_fd(), libc::F_GETPIPE_SZ);
            if got > 0 {
                got as usize
            } else {
                64 * 1024
            }
        }
    };
    Ok(Pipe { r, w, cap })
}

/// 封装 libc::splice。返回移动的字节数;0 表示 fd_in 到达 EOF;-1 转 io::Error
/// (EAGAIN 会被映射为 ErrorKind::WouldBlock,供 try_io 清就绪标志)。
fn raw_splice(fd_in: RawFd, fd_out: RawFd, len: usize) -> io::Result<usize> {
    // SPLICE_F_MOVE:提示内核移动页而非拷贝;SPLICE_F_NONBLOCK:splice 自身非阻塞,
    // 与 socket 的 O_NONBLOCK 无关,配合 readable/writable 就绪事件驱动。
    let ret = unsafe {
        libc::splice(
            fd_in,
            std::ptr::null_mut(),
            fd_out,
            std::ptr::null_mut(),
            len,
            (libc::SPLICE_F_MOVE | libc::SPLICE_F_NONBLOCK) as libc::c_uint,
        )
    };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret as usize)
    }
}

/// 单方向零拷贝:src socket → pipe → dst socket。返回累计转发字节。
///
/// 用 select! 并发等 src 可读 / dst 可写,避免「src 静默但 pipe 有 pending 待刷」
/// 时顺序 await 读端造成的死锁。字节只在 splice-out(pipe→dst 写出)侧计数:与 pump
/// 的写出侧口径一致,且异常中断时 pipe 残留不计入,反映实际送达 dst 的字节。
async fn splice_one(src: &TcpStream, dst: &TcpStream, counted: &AtomicI64) -> io::Result<u64> {
    let pipe = make_pipe()?;
    let pipe_w = pipe.w.as_raw_fd();
    let pipe_r = pipe.r.as_raw_fd();
    let src_fd = src.as_raw_fd();
    let dst_fd = dst.as_raw_fd();

    let mut pending: usize = 0; // pipe 内已填充但未排空的字节
    let mut eof = false; // src 是否已 EOF
    let mut total: u64 = 0;

    loop {
        // 终止:src 关闭且 pipe 排空 → 半关 dst 写端发 FIN,本方向结束。
        if eof && pending == 0 {
            // SAFETY:dst_fd 在 dst 存活期间有效;shutdown 失败(对端已关)无害,忽略。
            unsafe {
                libc::shutdown(dst_fd, libc::SHUT_WR);
            }
            return Ok(total);
        }

        let can_read = !eof && pending < pipe.cap;
        let can_write = pending > 0;

        tokio::select! {
            // 读分支:pipe 有空间且未 EOF 时启用。
            r = src.readable(), if can_read => {
                r?;
                // 就绪后内层批量 splice-in,直到 pipe 满或 socket 排空(减少 select! 往返)。
                // 关键:splice 的 EAGAIN 有两种不可区分的原因——
                //   (a) socket 缓冲空 → 应清读就绪,等下一个数据事件;
                //   (b) pipe 已满(按页/slot 计,字节数可能远未达 cap)→ socket 里可能仍有数据,
                //       此时绝不能清读就绪(mio 边缘触发下会 stall 到下个包才恢复)。
                // 用「pending < cap 时才继续读」区分:循环因 pending>=cap 退出 → pipe 满 →
                // 返回 Ok 不清就绪;循环因 raw EAGAIN 退出 → socket 空 → 返回 Err 让 try_io 清就绪。
                let res = src.try_io(Interest::READABLE, || {
                    while pending < pipe.cap {
                        match raw_splice(src_fd, pipe_w, pipe.cap - pending) {
                            Ok(0) => { eof = true; return Ok(()); }
                            Ok(n) => pending += n,
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Err(e),
                            Err(e) => return Err(e),
                        }
                    }
                    // pipe 满:保留读就绪,排空后下轮直接续读,不 stall。
                    // 取舍:每个「填满→部分排空」周期会多一次「立即就绪→EAGAIN→清就绪」的
                    // 有限空转 syscall,自限(一次即清,非忙循环),换来消除 stall,净收益为正。
                    Ok(())
                });
                match res {
                    Ok(()) => {}
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }
            }
            // 写分支:pipe 有数据时启用。
            w = dst.writable(), if can_write => {
                w?;
                // 就绪后内层批量 splice-out,直到 pipe 排空或 socket 不可写。
                // pipe 排空 → 返回 Ok 保留写就绪;socket 不可写(EAGAIN)→ 返回 Err 清写就绪。
                let res = dst.try_io(Interest::WRITABLE, || {
                    while pending > 0 {
                        match raw_splice(pipe_r, dst_fd, pending) {
                            Ok(0) => return Ok(()), // 仅防御:pipe_r 在 pending>0 时不会真 EOF
                            Ok(n) => {
                                pending -= n;
                                total += n as u64;
                                counted.fetch_add(n as i64, Ordering::Relaxed);
                            }
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Err(e),
                            Err(e) => return Err(e),
                        }
                    }
                    Ok(()) // pipe 排空:保留写就绪。
                });
                match res {
                    Ok(()) => {}
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }
            }
        }
    }
}

/// 双向零拷贝桥接。client 与 server 各被两个方向以 &TcpStream 共享
/// (readable/writable/try_io 均取 &self,tokio 读写就绪分离,可并发)。
pub async fn splice_bidi(
    client: TcpStream,
    server: TcpStream,
    counter: Arc<RuleCounter>,
) -> Result<()> {
    // 字段命名约定与 pump 一致:client→server 计 tx,server→client 计 rx。
    let c2s = splice_one(&client, &server, &counter.tx_bytes);
    let s2c = splice_one(&server, &client, &counter.rx_bytes);
    tokio::try_join!(c2s, s2c)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicI64;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// 起一个回显 server,经 splice_one 单向把 client 数据搬到 echo,验证字节计数与送达。
    #[tokio::test]
    async fn splice_one_forwards_and_counts() {
        // upstream:收到什么原样收下,统计总字节。
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let up_addr = upstream.local_addr().unwrap();
        let up_task = tokio::spawn(async move {
            let (mut s, _) = upstream.accept().await.unwrap();
            let mut buf = vec![0u8; 1 << 20];
            let mut got = 0usize;
            loop {
                let n = s.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                got += n;
            }
            got
        });

        // 本地两端:src 写入端 / dst 连 upstream。
        let src_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let src_addr = src_listener.local_addr().unwrap();
        let mut writer = TcpStream::connect(src_addr).await.unwrap();
        let (src, _) = src_listener.accept().await.unwrap();
        let dst = TcpStream::connect(up_addr).await.unwrap();

        let counted = AtomicI64::new(0);
        let payload = vec![7u8; 512 * 1024];
        let plen = payload.len();
        let pump = async {
            let n = splice_one(&src, &dst, &counted).await.unwrap();
            assert_eq!(n as usize, plen, "splice 转发字节数应等于发送量");
        };
        let feed = async {
            writer.write_all(&payload).await.unwrap();
            writer.shutdown().await.unwrap();
        };
        tokio::join!(pump, feed);

        let received = up_task.await.unwrap();
        assert_eq!(received, plen, "upstream 应收到全部字节");
        assert_eq!(counted.load(Ordering::Relaxed) as usize, plen, "计数应等于转发字节");
    }
}
