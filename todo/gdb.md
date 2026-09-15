# GDB 学习笔记：从 QEMU 远程调试到 Device Mapper 内核路径

> 练习目标：把 `strace` 看到的用户态 syscall 边界，接到 Asterinas 内核中实际执行的函数、分支和用户 buffer 写回路径。本文只记录实际验证过的操作与待验证项；用户可见 syscall 证据见 [strace.md](strace.md)。

## 1. GDB 在本项目中的位置

```text
容器终端 B：GDB 客户端 + 带符号内核文件
        │ TCP :1234，GDB Remote Serial Protocol
        ▼
容器终端 A：QEMU GDB stub
        │ 暂停/继续 vCPU，设置硬件断点，读写 guest 内存与寄存器
        ▼
QEMU 内：Asterinas kernel + NixOS guest + dmsetup
```

`strace` 只能证明用户态发出了何种 syscall；QEMU 远程 GDB 用于继续观察 syscall 进入内核后的实际分派。

```text
strace
→ ioctl(3</dev/mapper/control>, DM_VERSION, dm_ioctl buffer)

GDB
→ syscall_dispatch
→ sys_ioctl
→ inode_handle::ioctl
→ DmControlFile::ioctl
→ DM ABI 解码、buffer 读取、handler、写回
```

QEMU 的 `-S` 会让 vCPU 在复位后暂停。正确顺序是先连接 GDB、设置断点、再 `continue` 放行 guest；否则 guest 启动和目标命令可能在断点建立前已执行完。

## 2. 启动前准备

### 2.1 为什么 release 镜像不能按 Rust 源码断点

默认 `RELEASE=1` 的 NixOS 内核产物可能没有 DWARF 调试符号。此时 GDB 虽能连接 QEMU，却会显示：

```text
No debugging symbols found in .../asterinas-osdk-bin
No symbol table is loaded
```

它无法将 `device_mapper.rs:210` 映射到内核机器地址。需要构建并安装 dev profile 的调试镜像：

```bash
# 容器内 /root/asterinas
make nixos RELEASE=0 LOG_LEVEL=error
```

| 设置 | 作用 | 边界 |
|---|---|---|
| `RELEASE=0` | 使用 dev profile，保留源码行号、函数和类型的调试信息 | dev 与 release 是不同构建产物，首次构建可能较慢。 |
| `LOG_LEVEL=error` | 保持 guest 控制台安静 | 不影响 GDB 的断点或单步能力。 |

`make nixos` 会把新内核安装进 `target/nixos/asterinas.img`。它会覆盖该 guest 的运行时系统状态；不应在需要保留 guest 文件、LVM 元数据或实验数据时直接执行。

不要把 `LOG_LEVEL=info` 作为普通 GDB 前置条件：它会打开全内核所有 `ostd::info!` 日志，启动期输出会淹没 virtconsole。当前调试内核额外提供带 `[dm-debug]` 前缀的低频 DM 控制面日志；在 `LOG_LEVEL=error` 下可见，适合先关联 `dmsetup` 命令与内核 ioctl，再按需要进入 GDB。

可用下列命令确认当前调试内核包含符号：

```bash
readelf -S target/osdk/asterinas/asterinas-osdk-bin \
  | grep -E '\\.debug_(info|line)|\\.symtab'
```

预期至少能看到 `.debug_info`、`.debug_line` 和 `.symtab`。

### 2.2 `[dm-debug]` 控制面学习日志

在已构建的 `LOG_LEVEL=error` 调试镜像中，DM 控制 ioctl 会输出短小的 `[dm-debug]` 日志：`ioctl begin`、已解析 header、`ioctl done`，以及具有独立 ABI 或资源价值的控制提交与 core 状态提交。`dmsetup targets` 的 `DM_LIST_VERSIONS` 还会在每条完整 target-version record 的 `next` 最终回填后输出 `offset`、`next`、version、name 和 `record_len`，并以 `records`/`buffer_full` 收尾；`dmsetup target-version <name>` 只在成功编码时输出一条 `next=0` record 摘要。它们帮助关联 CLI 列表与可变长 record，但不替代 GDB 对原始用户 buffer 字节的独立验证。启动期不会因该通道打印 DM 日志；`DmDevice::enqueue`、target 映射、BIO split、completion 和 flush fan-out 均不记录每 I/O 日志。

一条 CLI 命令不等于一条 ioctl。libdevmapper 常先发送 `DM_VERSION`，例如 `dmsetup targets` 实测为 `DM_VERSION → DM_LIST_VERSIONS`；zero mapper 的 `create → load → resume` 分别对应 create、table load 与 resume ioctl。学习时在 guest shell 使用 marker 把命令和内核日志按时间关联：

```bash
printf 'DM_CMD_BEGIN targets\n'
dmsetup targets
printf 'DM_CMD_END targets\n'
```

2026-09-15 的 zero mapper 验证确认：load 会记录 ABI 请求中的 target 数、readonly 与 primary 发布结果，DM core 记录 inactive table 的 target 数和容量；首次 resume 记录 alias 发布后的 initial-resume commit；suspend、remove 记录控制提交与对应 core 状态。`DM_TABLE_CLEAR` 不再输出与 core 重复的 control committed 行，只有 core `state table-cleared` 说明 `cleared` 事实。zero mapper 的一次 Read/Write 在 `DM_CMD_BEGIN zero-io` 与 `DM_CMD_END zero-io` 之间没有 `[dm-debug]` BIO 日志。

预期 errno 也会有同一组 ioctl begin/header/done 记录，done 以 errno 收尾；这是本地学习通道的可见性选择，不代表额外的内核故障。`DM_DEV_WAIT` 目前仍在通用 dispatch 中异常，最多观察到其入口日志，不将其视为完成路径。

### 2.3 启动 QEMU GDB stub

先确认当前 guest 已正常退出，避免并行 QEMU 竞争 NixOS 镜像。然后在容器终端 A 执行：

```bash
cd /root/asterinas
QEMU_BIN='qemu-system-x86_64 -gdb tcp::1234 -S' make run_nixos
```

| 参数 | 含义 |
|---|---|
| `-gdb tcp::1234` | QEMU 在容器 TCP 1234 端口提供 GDB stub。 |
| `-S` | vCPU 从复位地址暂停，等待 GDB 放行。 |

终端 A 停住且没有 guest shell 输出是正常现象；保持该终端运行。

### 2.4 连接 GDB

在容器终端 B 执行：

```bash
cd /root/asterinas/kernel
cargo osdk debug --remote :1234
```

`cargo osdk debug` 会加载与当前构建一致的内核二进制，并让 GDB 连接到 QEMU stub。连接后看到：

```text
Remote debugging using :1234
0x000000000000fff0 in ?? ()
```

是正常的：`0xfff0` 是尚未启动的 x86 复位地址，不是 Asterinas 源码位置。

## 3. 设置与管理内核断点

### 3.1 Device Mapper 控制面入口

`/dev/mapper/control` 的 per-open ioctl 实现位于：

- [kernel/core/src/device/misc/device_mapper.rs](../kernel/core/src/device/misc/device_mapper.rs)
- `DmControlFile::ioctl` 入口：第 209 行附近。

启动后先设置硬件断点，再放行 guest：

```gdb
hbreak device_mapper.rs:210
continue
```

使用 `hbreak` 而不是普通 `break` 的原因：

```text
break
→ 软件断点，需要在目标地址写入断点指令。
→ guest 尚在复位阶段时，内核高地址映像可能尚不可访问，可能报：
  Cannot insert breakpoint.
  Cannot access memory at address ...

hbreak
→ 硬件断点，使用 QEMU/KVM 的硬件调试能力监视执行地址。
→ 无需在内核启动前改写目标内存。
```

硬件断点数量有限。一次命令练习结束后，应清理只服务于该命令的中间断点。

```gdb
info breakpoints       # 简写：i b；查看编号、状态、命中次数
delete 2               # 删除 2 号断点
disable 2              # 暂时停用
enable 2               # 重新启用
delete                 # 删除全部断点，确认 y
```

### 3.2 暂停、继续与退出

| 目标 | 命令 | 说明 |
|---|---|---|
| 从断点继续执行 | `continue` | 简写 `c`。目标继续运行直到下个断点、异常或退出。 |
| 在运行中的 GDB 终端取回提示符 | `Ctrl-C` | 暂停整个 guest，不杀死 QEMU。 |
| 让 guest 继续并解除调试连接 | `detach` | 删除/解除调试控制，让 QEMU guest 继续。 |
| 退出 GDB | `quit` | 通常在 `detach` 后执行。 |

```text
GDB 断在内核时
→ 整个 guest vCPU 暂停：dmsetup、shell、其他 guest 任务都会等待。

GDB continue 或 detach 后
→ guest 才继续运行。
```

若需要关闭 guest，应先让它运行，再在 guest 内执行：

```bash
poweroff
```

不要因为 GDB 连接异常就直接杀未知 QEMU 进程；先确认它是否为当前练习启动的实例。

## 4. GDB 内部命令与阅读规则

### 4.1 调用栈

```gdb
bt
```

`bt`（backtrace）从上到下列出当前帧及其调用者：

```text
#0  当前暂停位置
#1  直接调用 #0 的函数
#2  更早的调用者
```

因此实际执行方向从较低编号下方读向 `#0`。例如已验证的 DM ioctl 内核路径：

```text
syscall_dispatch(syscall_number=16)
→ sys_ioctl(raw_fd=3, cmd=..., arg=...)
→ inode_handle::ioctl
→ DmControlFile::ioctl
```

它与 `strace` 中的 `ioctl(3</dev/mapper/control>, ..., ...)` 对应：

```text
strace FD 3             ↔ sys_ioctl 的 raw_fd=3
strace ioctl command    ↔ sys_ioctl 的 cmd / RawIoctl.cmd
control device 的文件操作 ↔ DmControlFile::ioctl
```

`#4` 之后通常是用户任务调度、运行时和异常保护框架；首次定位具体 DM ioctl 时优先看 `#0` 至 syscall 分派层，不必被更深层的运行时帧分散注意。

### 4.2 单步：按语义块而非逐行

| 命令 | 作用 | 本训练的默认用法 |
|---|---|---|
| `next` | 执行当前源码行并停在下一行，尽量跨过被调函数 | 默认用于跨过已理解的辅助函数。 |
| `step` | 执行当前行并进入被调函数 | 仅在该函数内部逻辑本身是学习/排障目标时使用。 |
| `continue` | 运行到下一断点 | 误入宏、分配器、泛型或运行时细节时，设好外层检查点后直接使用。 |

Rust 的 `vec!`、泛型、内联和分配器会使一行源码展开为多层调用。例如：

```rust
let mut header = vec![0u8; DM_IOCTL_HEADER_SIZE];
```

单步后可能暂时显示到 `context.rs` 或 allocator 代码。这不表示 DM 语义进入了那些模块；应在外层目标行设置硬件断点并 `continue`，而不是逐层 `finish`。

源码行显示的规则：

```text
当前高亮行
→ 尚未执行，准备执行。

next 之后的新高亮行
→ 前一行已经执行完，局部变量结果可在此时打印。
```

例如：

```rust
let raw_cmd = raw_ioctl.cmd();
```

在该行停住时，`raw_ioctl` 已可打印，而 `raw_cmd` 还不应作为已产生的结果使用。执行 `next` 后再打印：

```gdb
p/x raw_cmd
```

它显示的就是上一行 `raw_ioctl.cmd()` 的返回值。

### 4.3 打印变量与内存

| 命令 | 含义 | 示例 |
|---|---|---|
| `p value` | 按默认格式打印表达式 | `p command` |
| `p/x value` | 按十六进制打印 | `p/x raw_cmd` |
| `x/3uw ADDRESS` | 从地址读取 3 个无符号 4 字节 word | `x/3uw raw_ioctl.arg` |
| `x/s ADDRESS` | 将地址处内容按 NUL 结尾字符串显示 | `x/s record_name_address` |

`RawIoctl` 的源码访问与 GDB 观察需要区分：

```text
Rust 源码
→ cmd() / arg() 是公开 accessor。

GDB
→ 可能按调试类型布局显示其私有存储字段；这是观察实现细节，
  不是可依赖的 Rust API。
```

远程内核调试中不要为了观察而调用任意 Rust 方法（表达式末尾有 `()`），包括 `raw_ioctl.cmd()` / `arg()`。这需要目标 CPU 执行额外指令，可能破坏暂停现场或导致 GDB 断开；优先读取已保存的局部变量，例如 `raw_cmd`，或从已确认的用户 buffer 地址读取最小 ABI 字段。

不要整体打印 `Vec<u8>` 或完整 `buffer`/`header`：GDB 会展开 Rust allocator、指针、容量和泛型类型，输出很长却不直接表达 DM ABI。只打印当前命令的最小字段集；需要看 ABI 内容时，用 `x` 从 `raw_ioctl.arg` 指向的用户 buffer 读取固定 offset。

出现：

```text
--Type <RET> for more, q to quit, c to continue without paging--
```

时输入 `q` 只会停止本次分页输出并回到 GDB 提示符，不会退出 GDB 或控制 guest。

### 4.4 可选 TUI 源码窗口

```gdb
layout src
```

TUI 会持续显示当前行前后的源码，使“执行上一行后，观察其生成的局部变量”不必反复翻文件。

若输出拥挤或不想显示源码：

```gdb
tui disable
```

也可用 `Ctrl-x a` 在 TUI 与普通模式间切换。

## 5. Device Mapper 命令闭环

每个代表命令按下列五行记录。先由学习者根据用户语义预判，再运行命令和 GDB；记录只写实际证据，不把预期写成已证实事实。

| 行 | 要回答的问题 | 典型证据 |
|---:|---|---|
| 1 | 用户要完成什么、需要哪些前置状态？ | `dmsetup` CLI 行为与输出。 |
| 2 | 预判会访问什么节点、发送何种 ioctl、哪些字段或状态会变化？ | 执行前假设；允许标记不确定。 |
| 3 | 用户态实际做了什么？ | `strace` 中 FD 生命周期、ioctl、`write`、errno。 |
| 4 | 内核实际走了什么路径？ | GDB `bt`、解码后的 command、最小输入/输出字段。 |
| 5 | 哪些结论已经闭环、还缺什么？ | 用户可见输出与内核写回/状态的对应。 |

### 5.1 `dmsetup version`

| 行 | 本次记录 |
|---:|---|
| 1 | 用户需要看到两类版本：Library version 是 libdevmapper 用户态自身信息；Driver version 需要向 DM control device 查询。无 mapper 前置状态。 |
| 2 | 预判打开 `/dev/mapper/control`，发出 `DM_VERSION`；libdevmapper 可能包含版本预检查和正式查询两次调用。 |
| 3 | `strace` 实际看到 control 节点检查、以 `O_RDWR` 打开 FD 3、读取 `/proc/devices`、两次 `DM_VERSION`、最后关闭 FD。Library version 的 `write` 发生在打开 control device 前；Driver version 的 `write` 发生在两次 ioctl 后。`strace -k` 显示第一次经过 `dm_task_create → dm_check_version → dm_task_run`，第二次直接从 `dm_driver_version → dm_task_run` 进入 ioctl。 |
| 4 | GDB 在 `DmControlFile::ioctl` 命中。`bt` 实际路径为 `syscall_dispatch → sys_ioctl(raw_fd=3) → inode_handle::ioctl → DmControlFile::ioctl`。读取到 `raw_cmd=0xc138fd00`、`command=0`、`data_size=16384`、`data_start=312`。回写后，同一用户 buffer 起始三项实际为 `4 48 0`。 |
| 5 | 已闭环：用户态 `DM_VERSION` 确实进入 DM control handler，内核把版本写入用户 buffer，dmsetup 据此输出 `Driver version: 4.48.0`。本次 GDB 未在写入前直接读取初始三项；`[4,0,0]` 仅有此前 strace 证据，不应标为本次 GDB 的已观察事实。 |

### 5.2 `dmsetup targets`

| 行 | 当前记录 |
|---:|---|
| 1 | 用户要列出当前内核支持的 target 类型及其版本。 |
| 2 | 预判 libdevmapper 先做 `DM_VERSION` 检查，核心请求为 `DM_LIST_VERSIONS`；内核需向 `data_start` 开始的可变长输出区写入 records。 |
| 3 | 已有 strace 看到 `DM_VERSION` 和 `DM_LIST_VERSIONS` 均发往 `/dev/mapper/control`；`TCGETS` 发往 stdout 的 `/dev/hvc0`，不是 DM ioctl。 |
| 4 | record 观察尚未完成。当前实现的 `list_versions()` 从 `data_start` 迭代 `SUPPORTED_TARGETS`，依次写入 `error`、`linear`、`striped`、`zero` 的版本 record。单条布局为：`next: u32`、`version[3]: u32`、NUL 结尾名称，并按 8 字节对齐。 |
| 5 | 待下次恢复：在 `DM_LIST_VERSIONS` 处理完成且写回后，从 `raw_ioctl.arg + data_start` 按 record 读取 `next`、版本与名称；不要整体打印 `buffer`。 |

对于当前四个 target，名称长度使每条 record 均为 24 字节。待验证时可使用：

```gdb
# 停在回写完成后的外层检查点
x/4uw raw_ioctl.arg + data_start
x/s raw_ioctl.arg + data_start + 16
```

第一条 record 的当前代码预期是：

```text
next = 24
version = 1.6.0
name = error
```

这只是由当前源码推导的待验证预期；恢复 GDB 练习时仍须以实际内存输出为准。

## 6. 工具选择边界

| 想回答的问题 | 优先工具 |
|---|---|
| dmsetup 是否打开了正确设备、发出何种 ioctl、收到何种 errno | `strace` |
| 某 syscall 是否真的进入某个 Asterinas Rust handler，实际走了哪个内核分支 | QEMU + GDB |
| 某个进程/FD 当前持有哪个对象 | `lsof`、`lsfd`、`/proc/<pid>/fd` |
| 用户态 libdevmapper 的普通函数逻辑和变量 | 用户态符号 + GDB；当前不作为 DM 内核主线默认步骤 |
| 高频路径、全局频率或性能热点 | `perf` 或内核 tracing；不能由少量 GDB 断点推断 |

推荐学习闭环：

```text
用户语义
→ 可证伪预判
→ strace 观察用户态/内核边界
→ GDB 观察关键内核分派或状态分支
→ 源码解释原因
→ 对照用户可见输出、errno 或后续状态
```

GDB 不是每条命令的必经步骤。对于路径短、`strace` 与源码已无歧义的命令，可只做 `strace + 源码走读`；当涉及 flags、状态机、可变长 buffer、失败路径或真实分支归属不确定时，再进入 GDB。
