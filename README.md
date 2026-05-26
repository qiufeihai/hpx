# hpx

个人自用的 Rust VLESS + HTTP/2 + TLS proxy server。

## 服务端

```bash
cargo run --release -- \
  --listen 0.0.0.0:443 \
  --cert /path/fullchain.pem \
  --key /path/privkey.pem \
  --host example.com \
  --path /path \
  --uuid 00000000-0000-0000-0000-000000000000
```

## 配置文件（推荐）

服务端支持使用 `--config` 读取一个简单的 `KEY=VALUE` 配置文件（按行解析，支持 `#` 注释）。

示例：`/etc/hpx/hpx.conf`

```ini
listen=0.0.0.0:443
cert=/etc/hpx/fullchain.pem
key=/etc/hpx/privkey.pem
host=example.com
path=/path
uuid=00000000-0000-0000-0000-000000000000
connect_timeout_ms=5000
idle_timeout_s=1800
sub_path=/sub
sub_token=change_me
public_host=example.com
public_port=443
```

多值写法（两种都支持）：

```ini
host=a.com,b.com
uuid=uuid1,uuid2
```

或：

```ini
host=a.com
host=b.com
uuid=uuid1
uuid=uuid2
```

启动：

```bash
hpx --config /etc/hpx/hpx.conf
```

优先级：
- 同一字段同时存在时：命令行参数会追加/覆盖配置文件
- `host/uuid` 会合并（配置文件 + 命令行）

## 订阅链接（可选）

服务端可以在同一个 H2+TLS 入口上额外提供订阅（只在你拉取订阅时用到，不影响转发链路）。通过配置开启：

- `sub_path`：订阅路径（示例 `/sub`）
- `sub_token`：订阅 token（建议设置，用于避免被扫到）
- `public_host/public_port`：订阅里写入的对外地址（不填会使用 `host` 与 `listen` 端口的组合）

访问示例：

- Clash Provider YAML：
  - `https://example.com/sub?token=change_me&fmt=clash`
- VLESS URI 列表（每行一个，便于 Shadowrocket/其它客户端导入）：
  - `https://example.com/sub?token=change_me&fmt=vless`

## RockyLinux 9 一键部署（推荐）

在 RockyLinux 9 上，从源码构建并以 systemd 部署。

```bash
sudo dnf -y install git
git clone <your_repo_url> hpx
cd hpx
sudo DOMAIN=zyko2.online bash scripts/rocky9_install_build_deploy.sh
```

DOMAIN 在首次安装时建议显式指定，避免脚本误用默认值；如果 `/etc/hpx/hpx.conf` 已存在，脚本会优先从其中的 `host=` 读取。
换句话说：
- 首次安装：必须显式指定 `DOMAIN=...`
- 后续重复执行脚本：只要 `hpx.conf` 还在，就可以不传 `DOMAIN`

脚本会生成：
- 配置文件：`/etc/hpx/hpx.conf`
- systemd：`/etc/systemd/system/hpx.service`

你需要把证书放到：
- 默认按 acme.sh ECC 路径读取（不会改动你的 acme.sh 目录）：
  - `/root/.acme.sh/<domain>_ecc/<domain>.key`
  - `/root/.acme.sh/<domain>_ecc/fullchain.cer`

并编辑 `/etc/hpx/hpx.conf` 里的这些字段：
- `host`：域名（要与证书匹配）
- `path`：H2 path（要与客户端一致）
- `uuid`：VLESS UUID（要与客户端一致）

如果你使用 acme.sh 默认路径（在 `/root/.acme.sh/...` 下），systemd 又是 `User=hpx` 运行，需要给 `hpx` 最小读取权限（脚本会自动尝试执行）：

```bash
setfacl -m u:hpx:--x /root
setfacl -m u:hpx:--x /root/.acme.sh
setfacl -m u:hpx:--x /root/.acme.sh/<domain>_ecc
setfacl -m d:u:hpx:r-- /root/.acme.sh/<domain>_ecc
setfacl -m u:hpx:r-- /root/.acme.sh/<domain>_ecc/<domain>.key
setfacl -m u:hpx:r-- /root/.acme.sh/<domain>_ecc/fullchain.cer
```

常用运维命令：

```bash
sudo systemctl restart hpx
sudo systemctl status hpx --no-pager -l
sudo journalctl -u hpx -e --no-pager
```

## 排障与日志

服务端默认日志级别由环境变量控制，常用方式：

```bash
RUST_LOG=info hpx --config /etc/hpx/hpx.conf
RUST_LOG=warn hpx --config /etc/hpx/hpx.conf
```

典型拒绝原因（服务端会打印结构化字段，不包含敏感 payload）：
- `unexpected alpn`：客户端未协商到 h2
- `path mismatch`：客户端 path 与服务端不一致
- `authority not allowed`：客户端 host 与服务端 allowlist 不一致
- `uuid not allowed`：UUID 不在 allowlist
- `connect failed`：到目标地址建立 TCP 失败

## Clash 示例

```yaml
proxies:
  - name: hpx
    type: vless
    server: example.com
    port: 443
    uuid: 00000000-0000-0000-0000-000000000000
    udp: false
    tls: true
    servername: example.com
    network: h2
    h2-opts:
      host:
        - example.com
      path: /path
```

## Clash.Meta（mihomo）建议：保持连接更稳

以下是全局 TCP keep-alive 的示例（更偏“稳定不断连”，但移动端更耗电）：

```yaml
keep-alive-interval: 15
keep-alive-idle: 15
disable-keep-alive: false
```

## Shadowrocket 提示

- 类型：VLESS
- 传输：HTTP/2
- TLS：开启
- Host：example.com
- Path：/path
- UUID：与服务端 `--uuid` 一致
