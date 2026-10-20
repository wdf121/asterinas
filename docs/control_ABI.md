# Device Mapper Control ABI 参考手册

本文用于查阅 Linux Device Mapper 控制接口的命令编码、结构体布局、字段含义和可变载荷格式。

**布局口径：**下文的数字布局与完整 ioctl 十六进制值采用 Linux **x86_64 ABI**；整数使用本机字节序，因此 x86_64 内存中为小端。不要把 ioctl 数值的书写顺序误认为内存字节顺序。其他体系结构需核对 ioctl 位段定义及 C 类型对齐。

**源码依据：**openEuler Linux 固定提交 `5078a3a23a1e1825ec136485173ec98668cdd640` 的 `include/uapi/linux/dm-ioctl.h`、`drivers/md/dm-ioctl.c` 和 `include/linux/kdev_t.h`。本文给出该版本的协议，不把不同版本新增的命令或字段混入同一张表。该头文件定义的 DM 接口版本为 **4.48.0**，与 Linux 内核版本不是同一概念。

## 1. 接口总览：命令号和参数缓冲区是两件事

用户程序通过控制字符设备发出管理请求：

```c
int fd = open("/dev/mapper/control", O_RDWR);
int ret = ioctl(fd, command, buffer);
```

| 参数 | 含义 |
|---|---|
| `fd` | `/dev/mapper/control` 的文件描述符；该节点的字符设备号为 10:236 |
| `command` | 32 位 ioctl 命令编码，包含方向、结构大小、类型和命令序号 |
| `buffer` | 用户态连续内存的地址；通常以 `struct dm_ioctl` 开头，后接命令专用数据 |

```text
ioctl(fd, command, buffer)
             │       │
             │       └── 用户缓冲区：公共头 + 可选载荷
             └────────── 命令编码：方向 + 大小 + 类型 + 序号
```

**命令编码不在 `dm_ioctl` 里面。** 同一缓冲区布局可以配不同命令，内核依据独立传入的 command 解释字段与载荷。

control 是管理入口，不是读写 mapper 数据的块设备。`dm_ioctl.dev` 表示被管理的块设备号，不是 control 的 10:236。

## 2. ioctl 命令的 32 位编码

### 2.1 位段划分

x86_64 使用以下划分：

```text
bit 31       30 29                         16 15            8 7             0
   +-----------+-----------------------------+---------------+---------------+
   | direction |            size             |     type      |      nr       |
   |   2 bits  |           14 bits            |    8 bits     |    8 bits     |
   +-----------+-----------------------------+---------------+---------------+
```

| 位段 | 位范围 | 掩码（右移前） | DM 定义 |
|---|---|---|---|
| direction | 31–30 | `0xc0000000` | `_IOWR`：值 3，二进制 `11` |
| size | 29–16 | `0x3fff0000` | `sizeof(struct dm_ioctl)` = 312 = `0x138` |
| type | 15–8 | `0x0000ff00` | `DM_IOCTL` = `0xfd`，二进制 `11111101` |
| nr | 7–0 | `0x000000ff` | 具体命令序号，见下一节 |

方向从**用户程序的角度**命名：WRITE 表示用户向内核提供数据，READ 表示内核向用户返回数据；`_IOWR` 同时声明两者。

所有下表中的标准 DM 命令都使用：

```c
_IOWR(0xfd, command_number, struct dm_ioctl)
```

所以它们的**高 24 位相同，只有低 8 位 nr 不同**：

```text
command = (3U << 30) | (312U << 16) | (0xfdU << 8) | nr
        = 0xc138fd00 | nr
```

以 VERSION 为例：

```text
方向   size（14 位）    type（8 位）  nr（8 位）
11     00000100111000   11111101     00000000

合并：11000001 00111000 11111101 00000000
数值：0xc138fd00
小端内存字节：00 fd 38 c1
```

注意：**编码中的 size 是公共结构体大小，不是整个请求的长度**。带可变载荷时，总长度放在 `dm_ioctl.data_size` 中。Linux DM 入口按 type/nr 识别命令，不额外用 direction/size 决定分派或用户内存复制长度。

### 2.2 完整命令编号表

`*_CMD` 是序号；不带 `_CMD` 的宏才是传给 `ioctl()` 的完整命令值。例如 `DM_TABLE_LOAD_CMD = 9`，但 `DM_TABLE_LOAD = 0xc138fd09`。

| ioctl 宏 | nr 十进制 | nr 二进制（8 位） | 完整命令值（x86_64） | 功能 |
|---|---:|---|---|---|
| `DM_VERSION` | 0 | `00000000` | `0xc138fd00` | 协商/查询接口版本 |
| `DM_REMOVE_ALL` | 1 | `00000001` | `0xc138fd01` | 移除设备集合 |
| `DM_LIST_DEVICES` | 2 | `00000010` | `0xc138fd02` | 列出设备 |
| `DM_DEV_CREATE` | 3 | `00000011` | `0xc138fd03` | 创建设备身份 |
| `DM_DEV_REMOVE` | 4 | `00000100` | `0xc138fd04` | 移除指定设备 |
| `DM_DEV_RENAME` | 5 | `00000101` | `0xc138fd05` | 改名或设置 UUID |
| `DM_DEV_SUSPEND` | 6 | `00000110` | `0xc138fd06` | 挂起或恢复设备 |
| `DM_DEV_STATUS` | 7 | `00000111` | `0xc138fd07` | 查询设备状态 |
| `DM_DEV_WAIT` | 8 | `00001000` | `0xc138fd08` | 等待设备事件并返回状态 |
| `DM_TABLE_LOAD` | 9 | `00001001` | `0xc138fd09` | 加载 inactive 表 |
| `DM_TABLE_CLEAR` | 10 | `00001010` | `0xc138fd0a` | 清除 inactive 表 |
| `DM_TABLE_DEPS` | 11 | `00001011` | `0xc138fd0b` | 查询表依赖设备 |
| `DM_TABLE_STATUS` | 12 | `00001100` | `0xc138fd0c` | 查询各 target 状态/配置 |
| `DM_LIST_VERSIONS` | 13 | `00001101` | `0xc138fd0d` | 列出 target 类型及版本 |
| `DM_TARGET_MSG` | 14 | `00001110` | `0xc138fd0e` | 向 target 发送消息 |
| `DM_DEV_SET_GEOMETRY` | 15 | `00001111` | `0xc138fd0f` | 设置兼容 CHS 几何信息 |
| `DM_DEV_ARM_POLL` | 16 | `00010000` | `0xc138fd10` | 重置当前 control fd 的全局事件基准 |
| `DM_GET_TARGET_VERSION` | 17 | `00010001` | `0xc138fd11` | 查询指定 target 类型版本 |

## 3. 公共头：`struct dm_ioctl`

### 3.1 C 定义

所有普通命令共用这个头，不为每个命令另设一个头部结构：

```c
#define DM_NAME_LEN 128
#define DM_UUID_LEN 129

struct dm_ioctl {
    __u32 version[3];
    __u32 data_size;
    __u32 data_start;
    __u32 target_count;
    __s32 open_count;
    __u32 flags;
    __u32 event_nr;
    __u32 padding;
    __u64 dev;
    char name[DM_NAME_LEN];
    char uuid[DM_UUID_LEN];
    char data[7];
};
```

`__u32` 为无符号 32 位整数、4 字节；`__s32` 为有符号 32 位整数；`__u64` 为无符号 64 位整数、8 字节。偏移单位均为**字节**，从整个结构起点 0 开始。

### 3.2 字段偏移表

| 字段 | 起始偏移 | 字节范围（含两端） | 大小 | 方向/含义 |
|---|---:|---|---:|---|
| `version[0]` | 0 | 0–3 | 4 | major，输入客户端版本，输出服务端版本 |
| `version[1]` | 4 | 4–7 | 4 | minor |
| `version[2]` | 8 | 8–11 | 4 | patch |
| `data_size` | 12 | 12–15 | 4 | 输入缓冲总容量；输出有效长度，包含公共头部分 |
| `data_start` | 16 | 16–19 | 4 | 可变数据相对整个结构起点的偏移 |
| `target_count` | 20 | 20–23 | 4 | 输入/输出 target 数，含义依命令而定 |
| `open_count` | 24 | 24–27 | 4 | 输出目标 mapper 的打开计数，不是 control fd 数 |
| `flags` | 28 | 28–31 | 4 | 输入选项和输出状态位 |
| `event_nr` | 32 | 32–35 | 4 | 事件序号；特定命令输入时复用为 udev cookie |
| `padding` | 36 | 36–39 | 4 | 显式填充 |
| `dev` | 40 | 40–47 | 8 | 用户 ABI 编码的目标块设备号 |
| `name[128]` | 48 | 48–175 | 128 | 设备名，部分 list 命令作过滤前缀 |
| `uuid[129]` | 176 | 176–304 | 129 | UUID，部分 list 命令作过滤前缀 |
| `data[7]` | 305 | 305–311 | 7 | 尾部 padding 或数据 |

```text
0           12            40      48                  176                  305 312
+-----------+-------------+-------+--------------------+--------------------+---+
| version   | 7 × 4 bytes | dev   | name[128]          | uuid[129]          | 7 |
+-----------+-------------+-------+--------------------+--------------------+---+
|<------------------- sizeof(struct dm_ioctl) = 312 -------------------------->|
|<---------------- offsetof(dm_ioctl, data) = 305 ------------------------->|
```

中间的 7 个四字节字段是 data_size、data_start、target_count、open_count、flags、event_nr、padding。

### 3.3 “头部多大”有三个不同答案

| 数字 | 定义 | 使用场景 |
|---:|---|---|
| **12** | `sizeof(version)` | 版本握手；VERSION 只访问这部分 |
| **305** | `offsetof(struct dm_ioctl, data)` | Linux 普通参数的最小固定前缀 |
| **312** | `sizeof(struct dm_ioctl)` | C 结构体总大小、标准 ioctl size 字段、常见的对齐后载荷起点 |

因此，不能笼统说“DM 所有命令都读 312 字节”。

`data[7]` 不是整个可变数据的容量上限。用户可以分配远大于 312 字节的连续缓冲区；可变记录通常放在偏移 312 或更后的地方，具体由 `data_start` 指定。

```text
buffer
├─ 偏移 0：公共头
├─ 偏移 305–311：尾部区域/填充
├─ 偏移 data_start：命令专用载荷
└─ 偏移 data_size：输入缓冲区结束（不包含该字节）
```

### 3.4 长度、偏移与计数的单位

- `data_size`：字节，包含公共头，不是“载荷字节数”。
- `data_start`：字节偏移，相对 buffer 起点，不是用户指针。
- `target_count`：记录数量，不是字节数。
- `dm_target_spec.next`：字节偏移，但其基点随输入/输出而变。
- `sector_start`、`length`、target message 的 `sector`：512 字节扇区单位，不是字节偏移。

输入容量与输出长度必须分开保存。命令完成后，`data_size` 可能已被改成回复长度，不能继续把它当作原始分配容量。

## 4. 各命令共用什么，又在哪里不同？

**相同：**标准命令编码的方向、size、type；普通请求的公共头布局；共同版本握手。

**不同：**nr、公共字段的用途、是否读取输入载荷、输出记录类型、flags 含义以及状态副作用。

下表描述数据形状，不列每个命令的全部业务校验。NO_PARAMS 是内核命令表属性，**不是用户 flags 中的一位**。

| 命令 | 主要公共头输入 | 额外输入载荷 | 主要输出 | NO_PARAMS |
|---|---|---|---|---|
| VERSION | 仅 version | 无 | 仅 version | 特殊路径 |
| REMOVE_ALL | flags | 无 | 公共头 | 是 |
| LIST_DEVICES | 容量、name/UUID 过滤字段 | 无结构化输入记录 | `dm_name_list` 链 | 否 |
| DEV_CREATE | name、可选 UUID、flags、可选 dev | 无 | 公共头，含 dev/状态 | 是 |
| DEV_REMOVE | selector、flags、cookie | 无 | 公共头 | 是 |
| DEV_RENAME | selector、flags、cookie | 新名称或新 UUID 的 NUL 字符串 | 公共头 | 否 |
| DEV_SUSPEND | selector、flags、cookie | 无 | 公共头 | 是 |
| DEV_STATUS | selector、查询 flags | 无 | 公共头状态 | 是 |
| DEV_WAIT | selector、event_nr、查询 flags、容量 | 无结构化输入记录 | 公共头 + target 状态记录 | 否 |
| TABLE_LOAD | selector、target_count、flags | `dm_target_spec` + 参数字符串，重复若干次 | 公共头 | 否 |
| TABLE_CLEAR | selector | 无 | 公共头 | 是 |
| TABLE_DEPS | selector、查询 flags、容量 | 无结构化输入记录 | `dm_target_deps` | 否 |
| TABLE_STATUS | selector、查询 flags、容量 | 无结构化输入记录 | `dm_target_spec` + 状态/配置文本 | 否 |
| LIST_VERSIONS | 容量 | 无结构化输入记录 | `dm_target_versions` 链 | 否 |
| TARGET_MSG | selector | `dm_target_msg` + 消息字符串 | 公共头，可附输出字符串 | 否 |
| DEV_SET_GEOMETRY | selector | NUL 结尾的 CHS 文本 | 公共头 | 否 |
| DEV_ARM_POLL | 无目标 selector 需求 | 无 | 公共头；事件基准更新在当前 fd 的内核私有状态中 | 是 |
| GET_TARGET_VERSION | name 指定 target 类型、容量 | 无结构化输入记录 | target version 记录 | 否 |

LIST_DEVICES 的 NO_PARAMS 为**否**。需要输出空间的命令，即使没有有意义的输入记录，也可能经过普通的整块参数复制路径；“没有输入载荷”不等于 NO_PARAMS。

### 4.1 selector

selector 指用于定位目标设备的公共字段：name、UUID 或 dev。

普通设备查找不应同时提供多个 selector：UUID 非空时不同时提供 name/dev；name 非空时不同时提供 dev；否则使用非零 dev。找到设备后，内核可把规范名称和 UUID 填回头部。

CREATE 不同：name 是待创建名称，UUID 可以一并指定；PERSISTENT_DEV 控制 dev 是否用于请求固定 minor。LIST 的名称/UUID 过滤也不是普通的“定位单一设备”。

### 4.2 active、inactive 与查询字段

- TABLE_LOAD 准备 **inactive** 表，不直接将其作为当前 I/O 映射。
- DEV_SUSPEND 在 SUSPEND 位置位时请求挂起，清零时用于 resume；resume 可以激活准备好的表。
- QUERY_INACTIVE_TABLE 让适用查询针对 inactive 表。
- ACTIVE_PRESENT、INACTIVE_PRESENT 分别表示两个槽是否存在。
- `target_count` 和 READONLY 等字段根据查询对象解释，不能仅看某一个存在标志推导所有状态。

表存在、设备挂起、块设备发布和表激活是不同概念，不能互相替代。

## 5. `flags`：32 位位图

`dm_ioctl.flags` 位于偏移 28，占 32 位。多个位用按位或组合，不是互斥枚举。

| 名称（省略 DM_ 前缀） | bit | 十六进制掩码 | 方向 | 含义 |
|---|---:|---|---|---|
| READONLY_FLAG | 0 | `0x00000001` | 入/出 | 只读模式 |
| SUSPEND_FLAG | 1 | `0x00000002` | 入/出 | 挂起请求或挂起状态 |
| PERSISTENT_DEV_FLAG | 3 | `0x00000008` | 入 | CREATE 指定 minor |
| STATUS_TABLE_FLAG | 4 | `0x00000010` | 入 | 请求 target 配置文本而非运行状态文本 |
| ACTIVE_PRESENT_FLAG | 5 | `0x00000020` | 出 | active 表存在 |
| INACTIVE_PRESENT_FLAG | 6 | `0x00000040` | 出 | inactive 表存在 |
| BUFFER_FULL_FLAG | 8 | `0x00000100` | 出 | 结果缓冲不足 |
| SKIP_BDGET_FLAG | 9 | `0x00000200` | 入 | 本基线忽略的兼容位 |
| SKIP_LOCKFS_FLAG | 10 | `0x00000400` | 入 | 挂起时跳过文件系统冻结尝试 |
| NOFLUSH_FLAG | 11 | `0x00000800` | 入 | 不走普通 flush 行为；含义还依命令/target 而定 |
| QUERY_INACTIVE_TABLE_FLAG | 12 | `0x00001000` | 入 | 查询 inactive 表 |
| UEVENT_GENERATED_FLAG | 13 | `0x00002000` | 出 | 已生成相应 uevent |
| UUID_FLAG | 14 | `0x00004000` | 入 | RENAME 设置 UUID 而不是改名 |
| SECURE_DATA_FLAG | 15 | `0x00008000` | 入 | 清理敏感用户缓冲及内核副本 |
| DATA_OUT_FLAG | 16 | `0x00010000` | 出 | target message 产生输出数据 |
| DEFERRED_REMOVE | 17 | `0x00020000` | 入/出 | 延迟删除请求/状态 |
| INTERNAL_SUSPEND_FLAG | 18 | `0x00040000` | 出 | 内部挂起状态 |
| IMA_MEASUREMENT_FLAG | 19 | `0x00080000` | 入 | 请求 IMA 测量信息格式 |

bit 2、7 和本表未定义的位不要自行赋予含义。Linux 对 flags 的处理是逐命令的，并非所有未知位都会统一被拒绝。

例：查询 inactive 表的配置文本可组合：

```text
QUERY_INACTIVE_TABLE_FLAG | STATUS_TABLE_FLAG
= 0x00001000 | 0x00000010
= 0x00001010
```

输入 flags 不应原样当作回复状态。普通参数处理会清除 BUFFER_FULL、UEVENT_GENERATED、SECURE_DATA、DATA_OUT 等位，再由后续处理生成输出。

## 6. 表记录：`struct dm_target_spec`

### 6.1 定义与布局

```c
#define DM_MAX_TYPE_NAME 16

struct dm_target_spec {
    __u64 sector_start;
    __u64 length;
    __s32 status;
    __u32 next;
    char target_type[DM_MAX_TYPE_NAME];
    /* A NUL-terminated parameter/status string follows. */
};
```

| 字段 | 相对记录起点偏移 | 大小 | 含义 |
|---|---:|---:|---|
| sector_start | 0 | 8 | target 在 mapper 中的逻辑起始 sector |
| length | 8 | 8 | 长度，单位为 512 字节 sector |
| status | 16 | 4 | 读取内核结果时使用，不是 ioctl 总返回值 |
| next | 20 | 4 | 下一条记录的位置，基点见下节 |
| target_type | 24 | 16 | target 类型，如 `linear` |
| 后续字符串 | 40 | 可变 | 参数或状态文本，包含结尾 NUL |

`sizeof(struct dm_target_spec) = 40`，x86_64 对齐要求为 8 字节。参数字符串不属于固定的 40 字节。

```text
一条记录：
+----------------------+------------------------+----------------+
| dm_target_spec：40 B  | 参数/状态文本 + NUL    | 对齐填充       |
+----------------------+------------------------+----------------+
```

首条 LOAD 记录从 `buffer + data_start` 开始，不应覆盖 312 字节的 dm_ioctl；后续记录不能覆盖上一条记录及其字符串。字符串必须在输入缓冲边界内找到 NUL。

### 6.2 最容易用错的 `next`

| 使用方向 | next 的基点 | 下一条记录地址 |
|---|---|---|
| TABLE_LOAD 输入 | **当前记录起点** | `current + current->next` |
| TABLE_STATUS / DEV_WAIT 输出 | **第一条记录起点** | `first + current->next` |

例如三条记录的总占用分别为 56、64、48 字节：

```text
相对 first：0          56             120          168
            | record 0 | record 1      | record 2    |

LOAD 输入：record 0.next = 56，record 1.next = 64
STATUS 输出：record 0.next = 56，record 1.next = 120
```

LOAD 按 `target_count` 消费记录，最后一条的 next 不会被用于读取下一条。不能把“最后 next 必须为零”当作通用要求。输出解析也要结合数量、实际 data_size 和 BUFFER_FULL，不能只依赖零终止。

### 6.3 一个完整 linear 请求的大小示例

映射含义：mapper 的 `[0, 2048)` sector 映射到 backing 8:16 的 `[4096, 6144)` sector。

人类可读表行：

```text
0 2048 linear 8:16 4096
```

转换为二进制后：

```text
公共头：
  data_start  = 312
  target_count = 1

记录：
  sector_start = 0
  length       = 2048
  status       = 0
  target_type  = "linear\0"，剩余字节清零
  参数字符串   = "8:16 4096\0"
```

参数字符串有 9 个可见字符，加 NUL 共 **10 字节**。所以：

```text
记录最少占用 = 40 + 10 = 50 字节
对齐占用     = align_up(50, 8) = 56 字节
缓冲总长度   = 312 + 56 = 368 字节
```

一个便于构造的完整布局是：

| buffer 偏移 | 内容 |
|---|---|
| 0–311 | dm_ioctl，data_size=368、data_start=312、target_count=1 |
| 312–351 | dm_target_spec，next 可设为记录对齐长度 56 |
| 352–361 | `8:16 4096` 加 NUL |
| 362–367 | 6 字节对齐填充 |

提交命令为 `DM_TABLE_LOAD = 0xc138fd09`。虽然 data_size 为 368，命令编码的 size 仍是 **312**，不会变成 368。

三个容易混淆的地址量：

- data_start=312：缓冲区内的**字节**偏移。
- sector_start=0：mapper 上的**逻辑扇区**起点。
- 参数 4096：backing 上的**扇区**起点。

## 7. 其他载荷结构

### 7.1 依赖数组：`dm_target_deps`

用于 TABLE_DEPS：

```c
struct dm_target_deps {
    __u32 count;
    __u32 padding;
    __u64 dev[];
};
```

| 内容 | 相对偏移 | 大小 |
|---|---:|---:|
| count | 0 | 4 |
| padding | 4 | 4 |
| dev[0] | 8 | 8 |
| dev[i] | `8 + 8*i` | 8 |

总有效长度为 `8 + 8*count`；dev 数组使用用户 ABI 设备号编码。必须检查乘法和加法溢出，不仅检查 count 非零。

### 7.2 设备名称记录：`dm_name_list`

用于 LIST_DEVICES：

```c
struct dm_name_list {
    __u64 dev;
    __u32 next;
    char name[];
};
```

| 内容 | 相对偏移 | 大小 |
|---|---:|---:|
| dev | 0 | 8 |
| next | 8 | 4 |
| name | 12 | 可变，包含 NUL |

**name 的偏移是 12，不是 `sizeof(struct dm_name_list)`。** 在 x86_64 中，该结构 sizeof 可因尾部对齐为 16；使用 sizeof 推导 name 起点会错过前四字节。

next 是相对当前记录的字节偏移；零表示名称链的末记录。空列表有专门的空记录表示，不能把返回区中的空名字当成一个真实设备。

该基线名称记录还带有扩展字段：从 name 的结尾 NUL 之后向上对齐到 8 字节边界，依次读取：

```text
u32 event_nr
u32 flags
可选 UUID 字符串（NUL 结尾）
```

这里的 flags 属于**名称记录**，不是公共头 flags：

| 名称记录 flag | 值 |
|---|---:|
| DM_NAME_LIST_FLAG_HAS_UUID | 1 |
| DM_NAME_LIST_FLAG_DOESNT_HAVE_UUID | 2 |

是否存在 UUID 以记录 flags 判断。遍历时同时遵守 next、返回长度和对应版本布局，不能无条件越过当前记录读取字符串。

### 7.3 target 版本记录：`dm_target_versions`

用于 LIST_VERSIONS 和 GET_TARGET_VERSION 的结果：

```c
struct dm_target_versions {
    __u32 next;
    __u32 version[3];
    char name[];
};
```

| 内容 | 相对偏移 | 大小 |
|---|---:|---:|
| next | 0 | 4 |
| version | 4 | 12 |
| name | 16 | 可变，包含 NUL |

next 相对当前记录，零表示链结束。这里的 version 是 target 类型的版本，例如 linear target 的版本，不是公共头中的 DM ioctl 版本。

### 7.4 target 消息：`dm_target_msg`

```c
struct dm_target_msg {
    __u64 sector;
    char message[];
};
```

sector 位于偏移 0，占 8 字节；message 从偏移 8 开始，为 NUL 结尾字符串。sector 用于选择设备地址范围内的 target，不是 message 在缓冲区中的偏移。

消息可能产生返回文本，DATA_OUT 表示存在输出；不能假定回复仍原封不动保留输入的消息结构。

### 7.5 纯字符串载荷

并非所有命令载荷都有独立结构体：

| 命令 | data_start 处的内容 |
|---|---|
| DEV_RENAME | 新名称；UUID_FLAG 置位时为新 UUID |
| DEV_SET_GEOMETRY | `"cylinders heads sectors_per_track start_sector\0"` |
GET_TARGET_VERSION 则不同：类型名称放在公共头 `dm_ioctl.name` 中，不在 data_start 指向的输入载荷中。

所有字符串都需要在给定缓冲边界内终止。内核接收的是字节串；不应擅自假定它们具有 Rust String 的 UTF-8 保证。

## 8. 设备号 `dev` 的二进制编码

`dm_ioctl.dev`、依赖数组中的 dev、名称记录中的 dev 使用用户 ABI 编码，不是简单的 `(major << 20) | minor` 内核内部值。

Linux 当前内部设备号范围为 12 位 major、20 位 minor，其 ABI 编码低 32 位如下：

```text
bit 31                 20 19                 8 7                 0
   +---------------------+--------------------+-------------------+
   | minor 的 bit 19..8  | major 的 bit 11..0 | minor 的 bit 7..0 |
   |       12 bits       |       12 bits      |       8 bits      |
   +---------------------+--------------------+-------------------+
```

编码公式：

```c
encoded = (minor & 0xff)
        | (major << 8)
        | ((minor & ~0xff) << 12);
```

在当前 Linux 有效设备号范围内，u64 字段高 32 位为零。

例：major=253、minor=256：

```text
内部值：       (253 << 20) | 256 = 0x0fd00100
用户 ABI 值：  (253 << 8) | (256 << 12) = 0x0010fd00
```

用户程序应使用 `<sys/sysmacros.h>` 的 `major()`、`minor()`、`makedev()`，不要自行用内部值填 dev。测试应包括 minor>255 的已知值，避免只测两个错误实现相互 encode/decode 也能通过的情况。

该 Linux 基线的 `huge_decode_dev(u64)` 最终调用 u32 解码函数，因此异常输入高 32 位被截断；这与支持扩展 64 位编码的库函数不一定完全相同。

## 9. 公共读写流程与错误优先级

### 9.1 版本握手是特殊的短路径

```text
权限检查
  → type 检查
  → 读取 12 字节 version
  → 比较客户端版本，暂存兼容性结果
  → 回写服务端 version
  → 传播兼容性结果
  → VERSION 直接成功；其他命令查找处理函数
```

兼容规则：major 相同，client minor ≤ server minor，patch 忽略。不兼容也会回写服务端版本；回写失败的 EFAULT 优先于不兼容的 EINVAL。

VERSION 不读取 data_size、data_start、flags 或字符串字段。其 ioctl 编码虽含 size=312，实际用户访问仍只有 **12 字节**。

未知 nr 也在版本握手后才查出：有权限、type 正确时，NULL 地址先产生 EFAULT，不兼容版本先产生 EINVAL，兼容版本握手完成后才返回 ENOTTY。错误 type 则在用户内存访问前返回 ENOTTY。

### 9.2 普通参数复制

找到非 VERSION 命令后：

1. 补读固定前缀 `[12, 305)`，复用已有版本数据。
2. 检查 data_size 上下限。
3. NO_PARAMS 命令将有效长度归一化为 305，不读取用户声明的大块尾部。
4. 其他命令分配有界缓冲，复用固定头，只补读 `[305, input_data_size)`。
5. 归一化 flags/固定字符串，再处理命令。

因此不要两次从用户内存读取整个头：用户线程可能在两次读取之间修改长度和字段，使校验对象与使用对象不一致。

### 9.3 输出容量与缓冲不足

输入 data_size 是容量，输出 data_size 是结果长度。Linux 分派普通命令前将输出长度初始设为 305，命令有结果载荷时再扩展。结果区通常从对齐后的 312 开始，并由 data_start 返回。

BUFFER_FULL 是需要检查的输出状态；ioctl 返回成功不一定意味着所有记录都已放入缓冲。target_count 也不能单独证明对应数量的记录都可读取，必须结合 flags 和有效边界。

### 9.4 失败不是统一“无副作用”

- 版本回写早于普通参数校验，因此命令失败时 version 也可能已改变。
- SECURE_DATA 会在执行命令前清理用户输入，并在释放时清理内核副本；NO_PARAMS 的清理范围按归一化后的 305 字节解释。
- 普通命令最终输出通常只在处理成功时复制；但状态变更可能已经发生，之后 copyout 仍可能失败。
- 用户内存复制故障可能发生在中途，不保证整个目标缓冲完全未改。

因此不能把所有 EFAULT 都解释成“设备状态没有变化”，也不能把所有失败都解释成“没有返回任何数据”。

## 10. 速查与布局自检

| 要查的内容 | 数值/规则 |
|---|---|
| DM type | `0xfd` |
| 命令编码 | 2 位 direction + 14 位 size + 8 位 type + 8 位 nr |
| 标准命令公共部分 | `0xc138fd00`，低 8 位替换为 nr |
| version | 偏移 0，12 字节 |
| 普通固定前缀 | 305 字节 |
| sizeof(dm_ioctl) | 312 字节 |
| 常用载荷起点 | 312，由 data_start 明确指定 |
| sizeof(dm_target_spec) | 40 字节，后接字符串 |
| dm_name_list.name | 相对记录偏移 12，不是 sizeof |
| dm_target_versions.name | 相对记录偏移 16 |
| dm_target_deps.dev | 相对记录偏移 8 |
| dm_target_msg.message | 相对记录偏移 8 |
| 字节序 | 本机字节序；x86_64 小端 |
| sector 单位 | 512 字节 |
| LOAD next | 相对当前 target 记录 |
| STATUS target next | 相对第一条 target 记录 |
| name/version 列表 next | 相对当前记录，零为链尾 |

可在 x86_64 用户态编译下面的断言，检查所用头文件是否与本文布局一致；这只验证布局，不执行控制命令：

```c
#include <stddef.h>
#include <linux/dm-ioctl.h>

_Static_assert(sizeof(struct dm_ioctl) == 312, "dm_ioctl size");
_Static_assert(offsetof(struct dm_ioctl, data) == 305, "fixed prefix");
_Static_assert(offsetof(struct dm_ioctl, dev) == 40, "dev offset");
_Static_assert(offsetof(struct dm_ioctl, name) == 48, "name offset");
_Static_assert(offsetof(struct dm_ioctl, uuid) == 176, "uuid offset");
_Static_assert(sizeof(struct dm_target_spec) == 40, "target spec size");
_Static_assert(offsetof(struct dm_name_list, name) == 12, "list name offset");
_Static_assert(offsetof(struct dm_target_versions, name) == 16, "version name offset");
_Static_assert(offsetof(struct dm_target_deps, dev) == 8, "deps offset");
_Static_assert(offsetof(struct dm_target_msg, message) == 8, "message offset");
_Static_assert(DM_VERSION == 0xc138fd00UL, "VERSION encoding");
_Static_assert(DM_TABLE_LOAD == 0xc138fd09UL, "TABLE_LOAD encoding");
```
