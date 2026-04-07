# autohour

用于自动提交 Linker 工时和日报，并支持检查某个月的缺报日期。

## 环境要求

- Rust / Cargo
- 可访问：
  - `https://login.linker.cc`
  - `https://lh.i.linker.cc`
  - `https://weeksystem.linker.cc`

## 环境变量

必填：

```bash
export LINKER_USERNAME='你的账号'
export LINKER_PASSWORD='你的密码'
export LINKER_PROJECT_ID='1799'
export AUTOHOUR_LOG_DIR='/你的日志目录'
```

可选：

```bash
export AUTOHOUR_SCHEDULE_AT='18:00'
export AUTOHOUR_HOLIDAY_DIR=''
export TELEGRAM_BOT_TOKEN=''
export TELEGRAM_CHAT_ID=''
export SMTP_HOST=''
export SMTP_PORT='587'
export SMTP_USERNAME=''
export SMTP_PASSWORD=''
export SMTP_FROM=''
export SMTP_TO=''
export SMTP_STARTTLS='true'
```

## 快速开始

直接提交当天日志：

```bash
cargo run --
```

提交指定日期日志：

```bash
cargo run -- --date 2026-04-02
```

检查当月缺报：

```bash
cargo run -- check-missing
```

检查指定年月缺报：

```bash
cargo run -- check-missing --year 2026 --month 4
```

启动 macOS 菜单栏应用：

```bash
cargo run -- tray
```

菜单栏模式会自动启用提醒：

- 上午 `08:00-10:00` 检查昨天是否已填报
- 下午 `18:00-20:00` 检查今天是否已填报
- 自动排除周末、法定节假日，保留调休工作日
- 未填报时每隔 `30` 分钟提醒一次
- 如果这两个时间段内电脑休眠，恢复后会补做一次漏掉的检查

前台定时执行：

```bash
cargo run -- daemon
```

指定每天执行时间：

```bash
cargo run -- daemon --at 18:30
```

仅刷新登录会话：

```bash
cargo run -- login
```

手动提交工时：

```bash
cargo run -- add-man-hour \
  --year 2026 \
  --month 4 \
  --day 2 \
  --workhour 8 \
  --project-id 1799 \
  --remark '工时：8'
```

编译后直接运行：

```bash
cargo build --release
./target/release/autohour
```

构建可分享的 macOS 应用：

```bash
./scripts/build-macos-app.sh
```

输出文件：

```text
dist/Autohour.app
dist/Autohour.app.zip
```

生成占位应用图标：

```bash
./scripts/generate-placeholder-icon.sh
```

## 日志文件要求

日志目录由 `AUTOHOUR_LOG_DIR` 指定，文件名必须是：

```text
YYYY-MM-DD.md
```

例如：

```text
2026-04-02.md
```

日志至少需要以下两个标题：

- `## 工作记录`
- `## 明日计划`

推荐格式：

```md
## 工作记录
工时：8
1. 完成 A
2. 处理 B

## 明日计划
继续推进 A，处理 B 的收尾问题

## 未完成工作
无

## 需协调工作
无

## 备注
无
```

`## 工作记录` 下必须包含一行工时声明，支持：

```text
工时: 8
工时：8
工时: 7.5h
工时：7.5h
```

## 缺报检测使用说明

检查缺报时会自动排除：

- 当天
- 周末
- 法定节假日

法定节假日配置文件放在：

```text
holidays/<year>.json
```

例如：

```text
holidays/2026.json
```

如果缺少对应年份的节假日文件，`check-missing` 会直接报错。

可用 `AUTOHOUR_HOLIDAY_DIR` 指定自定义节假日目录。

## 通知

配置 Telegram 或 SMTP 环境变量后，程序会在这些场景发送通知：

- 提交成功
- 提交失败
- 检测到缺报日期
- 菜单栏模式下也会显示 macOS 系统通知

## macOS App 使用

应用启动后会直接进入菜单栏模式。

应用会按以下顺序加载配置：

1. `AUTOHOUR_ENV_FILE` 指向的文件
2. 当前目录下的 `.env`
3. 可执行文件同目录下的 `.env`
4. `.app` 内 `Contents/Resources/.env`
5. `~/Library/Application Support/autohour/.env`

如果你要分享给别人使用，推荐做法：

- 把 `dist/Autohour.app.zip` 发给对方
- 对方解压后，把自己的配置写到 `~/Library/Application Support/autohour/.env`
- 节假日文件默认已随应用打包
- 如需开机自动启动，可在菜单栏里点“开启开机自动启动”

也可以命令行安装或卸载开机启动：

```bash
cargo run -- install-launch-agent
cargo run -- uninstall-launch-agent
```

## 常用命令

查看总帮助：

```bash
cargo run -- --help
```

查看某个子命令帮助：

```bash
cargo run -- check-missing --help
cargo run -- daemon --help
```

## 开发

检查编译：

```bash
cargo check
```

运行测试：

```bash
cargo test
```
