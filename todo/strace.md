# strace 学习笔记：从系统调用到 Device Mapper 实战

> 练习目标：掌握用 `strace` 将“用户态命令的现象”连接到“进程实际请求内核做了什么、内核返回什么”的方法。Asterinas Device Mapper 只是练习环境；方法可迁移到普通 CLI、服务、容器、网络和存储问题。

## 1. 什么时候使用 strace？

`strace` 适合回答这类问题：

```text
一个用户态进程实际调用了哪些 syscall？
它访问了什么文件、设备、socket？
它在哪个 syscall 上失败、errno 是什么？
它是否卡在某个 syscall？
某个 ioctl、read、write、connect 是否真的发到了内核？
```

典型场景：

| 现象 | strace 能提供的证据 |
|---|---|
| CLI 报“文件不存在” | `openat` / `newfstatat` 的路径与 `ENOENT`。 |
| 权限错误 | `openat` 返回 `EACCES` / `EPERM`。 |
| 设备命令失败 | `ioctl` 名称、FD 对象、输入/输出 buffer 与 errno。 |
| 网络客户端失败 | `socket`、`connect`、`sendto`、`recvfrom` 的返回值。 |
| 程序疑似卡住 | 最后一条阻塞的 `read`、`poll`、`futex`、`ioctl` 等。 |
| 子进程行为不明 | `-f` 跟踪 fork/exec 后的子进程或线程。 |

### 1.1 strace 的边界：其他场景该用什么？

| 你真正想知道的事 | 优先工具 | 原因 |
|---|---|---|
| 哪个进程持有文件、设备、socket 或 mount | `lsof`、`lsfd`、`/proc/<pid>/fd` | 直接观察 FD 持有关系。 |
| 进程内部变量、用户态函数逻辑、崩溃位置 | `gdb` | `strace` 看不到普通函数调用和内存。 |
| CPU 为什么高、热点函数在哪 | `perf` | syscall 时间不等于 CPU 热点。 |
| 内核中谁走了某条路径、频率是多少 | `bpftrace` / BCC / ftrace | strace 只看被跟踪进程的用户态 syscall 边界。 |
| 网络包是否真正到达 | `tcpdump` / Wireshark | `connect` 成功不等于应用协议一定正确。 |
| 容器/进程看见的挂载不同 | `lsns`、`nsenter`、`/proc/<pid>/mountinfo` | 先确认 namespace 视角。 |

原则：

```text
strace 不是“万能调试器”。

它擅长回答：进程向内核请求了什么、内核如何回应。
它不擅长回答：用户态或内核内部为什么这样实现。
```

## 2. strace 与 grep：常用参数和组合方式

### 2.1 先完整采集，再筛选

```bash
strace -f -tt -T \
  -o /tmp/example.strace \
  <command>
```

| 参数 | 含义 | 适用目的 |
|---|---|---|
| `-f` | 同时跟踪子进程和线程。 | 防止关键行为发生在子进程而漏掉。 |
| `-tt` | 每行输出微秒级时间戳。 | 判断先后顺序；不用于精确性能结论。 |
| `-T` | 显示 syscall 耗时。 | 发现明显阻塞点。 |
| `-o FILE` | 将 trace 写入文件。 | 避免 trace 淹没终端，便于反复筛选。 |

之后用 grep 按**对象**和**动作**筛选：

```bash
grep -nE '/dev/mapper/control|openat|ioctl|close' \
  /tmp/example.strace
```

| grep 部分 | 含义 |
|---|---|
| `-n` | 显示原始 trace 的行号，方便回看上下文。 |
| `-E` | 启用扩展正则表达式。 |
| `A|B|C` | 匹配 A、B、C 中任意一个模式。 |
| `/dev/mapper/control` | 按关键对象筛选。 |
| `openat` / `ioctl` / `close` | 按设备 FD 生命周期动作筛选。 |

这种方式适合第一次排障：保留完整证据，之后再改变筛选条件验证新假设。

### 2.2 从采集阶段就降噪

```bash
strace -f -tt -T -yy -s 256 \
  -e trace=ioctl,close \
  -o /tmp/example.strace \
  <command>
```

| 参数 | 含义 |
|---|---|
| `-e trace=ioctl,close` | 只记录 `ioctl` 与 `close`；其他 syscall 不写入 trace。 |
| `-yy` | 尽量将 FD 注释为实际对象或路径。 |
| `-s 256` | 字符串或 buffer 最多显示 256 字节，降低关键内容被截断的概率。 |

`-yy` 的价值例如：

```text
ioctl(3</dev/mapper/control<char 10:236>>, DM_VERSION, ...) = 0
```

无需回头搜索 `openat`，也可以直接确认：

```text
FD 3 对应 /dev/mapper/control。
```

### 2.3 需要知道“谁调用了 syscall”时

```bash
strace -f -tt -T -yy -k \
  -e trace=ioctl \
  -o /tmp/example-stack.strace \
  <command>
```

| 参数 | 含义 |
|---|---|
| `-k` | 每个被记录 syscall 后附加用户态调用栈。 |

调用栈从上到下的方向：

```text
最上面：离 syscall 最近的函数。
最下面：main、运行时入口等较早的调用者。
```

因此阅读调用链时通常从下向上：

```text
main
-> 某个用户态库函数
-> libc syscall wrapper
-> syscall
```

注意：`-k` 会引入明显追踪开销。它适合确认调用来源，不应用其时间差做精确性能结论。

## 3. 第一次见到 syscall：建立基础印象

以下四类 syscall 足以构成“用户态命令控制一个设备”的基本链条：

```text
路径检查
-> 打开设备
-> 通过 FD 控制设备
-> 关闭 FD
```

### 3.1 `newfstatat`：看路径是什么，不打开它

示例：

```text
newfstatat(AT_FDCWD, "/dev/mapper/control", {...}, 0) = 0
```

含义：

```text
进程询问：这个路径存在吗？它是普通文件、目录还是设备？权限和设备号是什么？
```

它的效果接近：

```bash
stat /dev/mapper/control
```

重点：

```text
newfstatat 成功
≠ 已经打开设备。

它只证明路径存在，且进程能读取元数据。
```

DM control 示例中：

```text
S_IFCHR
-> 字符设备。

0600
-> 仅 owner 可读写。

major 10, minor 236
-> control device 的设备号。
```

### 3.2 `openat`：真正打开并取得 FD

示例：

```text
openat(AT_FDCWD, "/dev/mapper/control", O_RDWR) = 3
```

含义：

```text
进程以读写方式打开 /dev/mapper/control；
内核成功返回 FD 3。
```

`FD 3` 不是 DM minor，也不是设备号；它只是这个进程内部引用已打开对象的编号。

`openat` 是比传统 `open` 更通用的接口：

```text
open(path)
-> 打开路径。

openat(dirfd, path)
-> 相对于某个目录 FD 打开路径。
```

当路径是绝对路径时：

```text
openat(AT_FDCWD, "/dev/mapper/control", ...)
```

实际效果接近：

```c
open("/dev/mapper/control", ...)
```

### 3.3 `ioctl`：通过 FD 请求设备做事

示例：

```text
ioctl(3, DM_VERSION, ...) = 0
```

含义：

```text
FD 3 对应已打开的 control device；
用户态通过它向内核发出 Device Mapper 控制请求；
0 表示该 syscall 成功。
```

常见 errno 的初步分类：

| 结果 | 初步含义 |
|---|---|
| `= 0` | syscall 成功。 |
| `= -1 ENOTTY` | 对应设备不支持或未识别该 ioctl。 |
| `= -1 ENXIO` | 设备不存在或当前不可访问。 |
| `= -1 EINVAL` | 参数、buffer、flag 或状态不合法。 |
| `= -1 EBUSY` | 设备/资源当前被占用或处于不允许的状态。 |

### 3.4 `close`：释放 FD

示例：

```text
close(3) = 0
```

含义：

```text
命令结束时关闭此前得到的 FD 3。
```

### 3.5 一条完整的设备控制链

```text
newfstatat("/dev/mapper/control") = 0
-> 节点存在，可读取元数据。

openat("/dev/mapper/control", O_RDWR) = 3
-> 设备成功打开，得到 FD 3。

ioctl(3, DM_..., ...) = 0
-> 通过 control FD 请求内核完成 DM 操作。

close(3) = 0
-> 释放该设备 FD。
```

从失败位置可以快速分层：

| 失败位置 | 首先怀疑 |
|---|---|
| `newfstatat(...)=ENOENT` | 路径、节点发布、挂载或命名空间。 |
| `openat(...)=EACCES` | 当前身份、owner/group、权限位。 |
| `openat(...)=ENODEV` | 节点存在，但对应内核设备不可用。 |
| `ioctl(...)=ENOTTY` | 已打开正确设备，但 ioctl 解码或实现不支持。 |

## 4. Device Mapper 实战观察

### 4.1 执行位置

Asterinas DM 的用户态命令运行在 QEMU 的 NixOS guest：

```text
Docker 容器
-> QEMU
-> Asterinas guest kernel
-> NixOS guest 用户态
-> dmsetup
-> /dev/mapper/control
```

因此观察 `dmsetup` 与 Asterinas DM 的 syscall 时，必须在 guest 中运行 `strace`。

从容器启动 guest：

```bash
# 在项目容器的 /root/asterinas 中
make run_nixos
```

进入：

```text
root@asterinas
```

后运行：

```bash
command -v strace
command -v dmsetup
```

`strace -o /tmp/x.strace` 写出的文件存在 guest 的 `/tmp`。要在容器终端看到内容，最简单的方式是让 guest 输出筛选结果：

```bash
grep -nE '...' /tmp/x.strace
```

guest stdout 会经 virtconsole、QEMU stdio 显示到运行 `make run_nixos` 的终端。

### 4.2 `dmsetup version`

执行：

```bash
strace -f -tt -T \
  -o /tmp/dmsetup-version.strace \
  dmsetup version
```

观察到的核心链：

```text
newfstatat("/dev/mapper/control", ...) = 0
openat("/dev/mapper/control", O_RDWR) = 3
ioctl(3, DM_VERSION, ...) = 0
close(3) = 0
```

得到的结论：

```text
- guest 中 /dev/mapper/control 存在，且是字符设备。
- dmsetup 成功取得 control FD。
- DM_VERSION 确实发往 Asterinas DM ioctl 控制面。
- Asterinas 返回的 driver ABI 版本是 4.48.0。
```

#### Library version 与 Driver version 的来源

`dmsetup version` 同时显示两类版本：

```text
Library version
-> 用户态 libdevmapper 自身携带的版本信息；不需要 ioctl。

Driver version
-> DM_VERSION ioctl 写回 dm_ioctl buffer 的版本信息。
```

trace 中：

```text
[{version=[4, 0, 0], ...}]
=>
[{version=[4, 48, 0], ...}]
```

表示 ioctl 前后同一用户态 buffer 的关键字段。`=>` 左边是请求时的内容，右边是内核写回后的内容。

#### 两次 `DM_VERSION`

使用：

```bash
strace -f -tt -T -yy -k \
  -e trace=ioctl \
  -o /tmp/dmsetup-version-stack.strace \
  dmsetup version
```

观察到两条不同用户态调用路径：

```text
第一次：
dmsetup main
-> _version
-> dm_driver_version
-> dm_task_create
-> dm_check_version
-> dm_task_run
-> ioctl(DM_VERSION)

第二次：
dmsetup main
-> _version
-> dm_driver_version
-> dm_task_run
-> ioctl(DM_VERSION)
```

当前可证实结论：

```text
两次 ioctl 不是 strace 重复记录；
它们来自 libdevmapper 的两个不同 call site。

第一次属于创建 task 时的 dm_check_version 路径；
第二次是 dm_driver_version 直接执行 task 的路径。
```

当前无需继续深究 `dm_check_version` 的精确内部规则。只有出现版本兼容错误、请求/响应不一致、性能异常，或准备修改 libdevmapper 时，才值得进一步进入 LVM2 用户态源码。

### 4.3 `dmsetup targets`

执行：

```bash
strace -f -tt -T -yy -s 256 \
  -e trace=ioctl,close \
  -o /tmp/dmsetup-targets.strace \
  dmsetup targets

cat /tmp/dmsetup-targets.strace
```

观察到：

```text
ioctl(3</dev/mapper/control<char 10:236>>, DM_VERSION, ...) = 0
ioctl(3</dev/mapper/control<char 10:236>>, DM_LIST_VERSIONS, ...) = 0
ioctl(1</dev/hvc0<char 229:0>>, TCGETS, ...) = 0
close(3</dev/mapper/control<char 10:236>>) = 0
+++ exited with 0 +++
```

结论：

```text
DM_VERSION
-> 该 dmsetup/libdevmapper 路径中的前置版本检查。

DM_LIST_VERSIONS
-> dmsetup targets 的核心语义：查询 target 类型与版本列表。

TCGETS on FD 1</dev/hvc0>
-> 查询 stdout 对应的 guest virtio console 终端属性；不是 DM ioctl。

+++ exited with 0 +++
-> dmsetup 正常退出，exit status 为 0。
```

#### `data_start` 与可变长输出

`DM_LIST_VERSIONS` 的请求包含：

```text
data_size=16384
data_start=312
```

含义：

```text
0 .. 311
-> 固定长度 dm_ioctl header。

312 .. 16383
-> 内核写入可变长度 target-version records 的输出区。
```

示意：

```text
┌───────────────────────────────┬──────────────────────────────┐
│ dm_ioctl 固定头                │ 可变长 target version records │
│ 0 .. 311                       │ 312 .. 16383                  │
└───────────────────────────────┴──────────────────────────────┘
                                  ^
                                  data_start = 312
```

trace 里的 `...` 只表示 strace 没有完整展开全部 record，不表示内核没有返回数据。

#### `/proc/devices` 与筛选边界

在 targets 的只采集 `ioctl,close` trace 中看到：

```text
close(4</proc/devices>) = 0
```

这说明 dmsetup/libdevmapper 之前打开并读取过 `/proc/devices`，但 `openat` 被 `-e trace=ioctl,close` 过滤掉了。

当前只记录事实：

```text
libdevmapper 会读取 guest kernel 的 /proc/devices；
它不是 DM control ioctl。
```

## 5. 后续学习计划

不需要机械 trace 所有用户态命令。应按不同语义类别选择代表命令。

### 5.1 查询类控制面

| 代表命令 | 预计核心 ioctl | 训练目标 |
|---|---|---|
| `dmsetup ls` | `DM_LIST_DEVICES` | 当前 mapper 实例发现；空列表成功与 ENXIO 的区别。 |
| `dmsetup info <name>` | `DM_DEV_STATUS` | selector、header、状态字段。 |
| `dmsetup table <name>` | `DM_TABLE_STATUS` | active/inactive table 查询。 |
| `dmsetup deps <name>` | `DM_TABLE_DEPS` | backing device dependency 的可变长输出。 |

### 5.2 生命周期类控制面

使用临时 mapper，例如 `strace_lab`：

| 命令 | 预计核心 ioctl | 训练目标 |
|---|---|---|
| `dmsetup create --notable strace_lab` | `DM_DEV_CREATE` | mapper identity 与 tableless 状态。 |
| `dmsetup load strace_lab ...` | `DM_TABLE_LOAD` | target spec 与可变长输入 buffer。 |
| `dmsetup resume strace_lab` | `DM_DEV_SUSPEND`，不带 suspend flag | 命令名称与 ioctl/flag 语义的差异。 |
| `dmsetup suspend strace_lab` | `DM_DEV_SUSPEND` + `DM_SUSPEND_FLAG` | 生命周期切换。 |
| `dmsetup clear strace_lab` | `DM_TABLE_CLEAR` | inactive table 清理。 |
| `dmsetup remove strace_lab` | `DM_DEV_REMOVE` | runtime 资源撤销与 errno。 |

### 5.3 失败路径

目标是将 dmsetup 错误文本映射到真实 syscall 与 errno：

```text
不存在 mapper -> ENXIO
重复 create -> EEXIST
同名 rename -> EBUSY
busy remove -> EBUSY
未支持 command -> ENOTTY / EOPNOTSUPP
```

### 5.4 等待与数据面

```text
dmsetup wait
-> 练习判断 ioctl 是否阻塞，timeout 如何终止等待。

dd / blkdiscard / fsync
-> 练习 open/read/write/fsync/ioctl 的用户态边界。
```

注意：strace 能看到用户态发起 I/O，却不能直接说明 DM target 如何 remap/split BIO。要进一步理解数据面，需要结合内核日志、ktest，以及后续的 perf/eBPF。

## 6. 每次练习记录模板

````markdown
## 命令

```bash
# 实际执行的命令
```

## 预期用户语义

-

## 关键 FD

-

## syscall / ioctl 时间线

```text
```

## 输入/输出 buffer 的关键字段

-

## errno / 退出状态

-

## 已证实结论

-

## 未证实假设与下一步

-
````
