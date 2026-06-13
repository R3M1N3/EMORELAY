-- P1(对标 flux 协议屏蔽): 节点级协议嗅探阻断位掩码。
-- 防端口转发被滥用为开放 HTTP/SOCKS 代理或套 CDN。bit0=http(1) bit1=tls(2)
-- bit2=socks(4);0=不阻断(默认,存量节点行为不变)。仅普通 TCP relay 生效。
-- 属防滥用(被动首包指纹+断连),非攻击类功能。
ALTER TABLE nodes ADD COLUMN block_protocols INTEGER NOT NULL DEFAULT 0;
