"""开发期 UDP echo server，单元 J 端到端测试用。
绑定 0.0.0.0:9998，echo 每个 datagram 回原 client_addr。
"""
import socket
import sys


def main() -> None:
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 9998
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind(("0.0.0.0", port))
    print(f"udp echo listening on 0.0.0.0:{port}", flush=True)
    try:
        while True:
            data, addr = s.recvfrom(65535)
            s.sendto(data, addr)
    except KeyboardInterrupt:
        pass
    finally:
        s.close()


if __name__ == "__main__":
    main()
