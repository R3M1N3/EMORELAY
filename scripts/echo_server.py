"""开发期 TCP echo server，单元 I 端到端测试用。
监听 0.0.0.0:9999，每个连接 echo 回所有收到的字节。
"""
import socket
import threading
import sys


def serve(conn: socket.socket, addr) -> None:
    try:
        while True:
            data = conn.recv(4096)
            if not data:
                break
            conn.sendall(data)
    except OSError:
        pass
    finally:
        conn.close()


def main() -> None:
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 9999
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("0.0.0.0", port))
    s.listen(64)
    print(f"echo listening on 0.0.0.0:{port}", flush=True)
    try:
        while True:
            conn, addr = s.accept()
            t = threading.Thread(target=serve, args=(conn, addr), daemon=True)
            t.start()
    except KeyboardInterrupt:
        pass
    finally:
        s.close()


if __name__ == "__main__":
    main()
