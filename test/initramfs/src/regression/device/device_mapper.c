// SPDX-License-Identifier: MPL-2.0

/* Raw DM ABI, node rollback, table modes, and event-lifecycle regressions.
 * These tests exercise the control fd rather than a userspace DM wrapper.
 */
#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <linux/dm-ioctl.h>
#include <linux/fs.h>
#include <poll.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <signal.h>
#include <sys/ioctl.h>
#include <sys/mount.h>
#include <sys/mman.h>
#include <stddef.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <sys/wait.h>
#include <unistd.h>

#include "../common/test.h"

#define DM_CONTROL_PATH "/dev/mapper/control"
#define DM_IOCTL_ENVELOPE_SIZE 312
#define DM_TABLE_BUFFER_SIZE 512
#define DM_IOCTL_FIXED_SIZE 305

union dm_table_buffer {
	struct dm_ioctl align;
	unsigned char bytes[DM_TABLE_BUFFER_SIZE];
};

static void init_ioctl(struct dm_ioctl *io)
{
	memset(io, 0, sizeof(*io));
	io->version[0] = DM_VERSION_MAJOR;
	io->version[1] = DM_VERSION_MINOR;
	io->version[2] = DM_VERSION_PATCHLEVEL;
	io->data_size = sizeof(*io);
	io->data_start = sizeof(*io);
}

static void init_named_ioctl(struct dm_ioctl *io, const char *name)
{
	init_ioctl(io);
	snprintf(io->name, sizeof(io->name), "%s", name);
}

static size_t align_to_u64(size_t value)
{
	return (value + sizeof(uint64_t) - 1) & ~(sizeof(uint64_t) - 1);
}

/* Raw readers avoid alignment assumptions for guard-page ioctl arguments. */
static uint32_t raw_u32(const unsigned char *bytes)
{
	uint32_t value;
	memcpy(&value, bytes, sizeof(value));
	return value;
}

static uint64_t raw_u64(const unsigned char *bytes)
{
	uint64_t value;
	memcpy(&value, bytes, sizeof(value));
	return value;
}

static int bytes_are(const unsigned char *bytes, size_t len,
		     unsigned char value)
{
	for (size_t i = 0; i < len; i++) {
		if (bytes[i] != value)
			return 0;
	}
	return 1;
}

static void init_linear_table(unsigned char *buffer, size_t buffer_size,
			      const char *name, uint64_t length)
{
	struct dm_ioctl *io = (struct dm_ioctl *)buffer;
	struct dm_target_spec *target;
	char *params;
	size_t record_size;

	memset(buffer, 0, buffer_size);
	init_named_ioctl(io, name);
	target = (struct dm_target_spec *)(buffer + io->data_start);
	target->sector_start = 0;
	target->length = length;
	target->next = 0;
	snprintf(target->target_type, sizeof(target->target_type), "linear");
	params = (char *)(target + 1);
	snprintf(params, buffer_size - io->data_start - sizeof(*target),
		 "/dev/vdc 0");
	record_size = sizeof(*target) + strlen(params) + 1;
	io->target_count = 1;
	io->data_size = io->data_start + align_to_u64(record_size);
}

static void init_zero_table(unsigned char *buffer, size_t buffer_size,
			    const char *name, uint64_t length)
{
	struct dm_ioctl *io = (struct dm_ioctl *)buffer;
	struct dm_target_spec *target;
	size_t record_size;

	memset(buffer, 0, buffer_size);
	init_named_ioctl(io, name);
	target = (struct dm_target_spec *)(buffer + io->data_start);
	target->sector_start = 0;
	target->length = length;
	target->next = 0;
	snprintf(target->target_type, sizeof(target->target_type), "zero");
	record_size = sizeof(*target) + 1;
	io->target_count = 1;
	io->data_size = io->data_start + align_to_u64(record_size);
}

static void init_rename_ioctl(unsigned char *buffer, size_t buffer_size,
			      const char *name, uint32_t flags,
			      const char *value)
{
	struct dm_ioctl *io = (struct dm_ioctl *)buffer;
	char *data;

	memset(buffer, 0, buffer_size);
	init_named_ioctl(io, name);
	io->flags = flags;
	data = (char *)(buffer + io->data_start);
	snprintf(data, buffer_size - io->data_start, "%s", value);
	io->data_size = io->data_start + align_to_u64(strlen(data) + 1);
}

struct dm_wait_result {
	int error;
	struct dm_ioctl header;
};

static void run_waiter(uint64_t dev, uint32_t event_nr, int ready_fd,
		       int result_fd)
{
	struct dm_wait_result result = { 0 };
	char ready = 1;
	int fd;

	fd = open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC);
	if (fd < 0)
		_exit(EXIT_FAILURE);
	init_ioctl(&result.header);
	result.header.dev = dev;
	result.header.event_nr = event_nr;
	if (write(ready_fd, &ready, sizeof(ready)) != sizeof(ready))
		_exit(EXIT_FAILURE);
	if (ioctl(fd, DM_DEV_WAIT, &result.header) < 0)
		result.error = errno;
	close(fd);
	if (write(result_fd, &result, sizeof(result)) != sizeof(result))
		_exit(EXIT_FAILURE);
	close(ready_fd);
	close(result_fd);
	_exit(result.error == 0 ? EXIT_SUCCESS : EXIT_FAILURE);
}

static pid_t spawn_waiter(uint64_t dev, uint32_t event_nr, int ready_pipe[2],
			  int result_pipe[2])
{
	pid_t pid;

	if (pipe(ready_pipe) < 0 || pipe(result_pipe) < 0)
		return -1;
	pid = fork();
	if (pid == 0) {
		close(ready_pipe[0]);
		close(result_pipe[0]);
		run_waiter(dev, event_nr, ready_pipe[1], result_pipe[1]);
	}
	if (pid < 0)
		return -1;
	close(ready_pipe[1]);
	ready_pipe[1] = -1;
	close(result_pipe[1]);
	result_pipe[1] = -1;
	return pid;
}

static void mapper_paths(uint64_t dev, const char *name, char *primary,
			 size_t primary_size, char *alias, size_t alias_size)
{
	snprintf(primary, primary_size, "/dev/dm-%u", minor((dev_t)dev));
	snprintf(alias, alias_size, "/dev/mapper/%s", name);
}

static void alarm_handler(int signal)
{
	(void)signal;
}

static int restart_signal_fd = -1;

static void restart_signal_handler(int signal)
{
	char delivered = 1;
	ssize_t write_result;
	int saved_errno = errno;

	(void)signal;
	if (restart_signal_fd >= 0) {
		write_result =
			write(restart_signal_fd, &delivered, sizeof(delivered));
		(void)write_result;
	}
	errno = saved_errno;
}

static int wait_fd_readable(int fd, int timeout_ms)
{
	struct pollfd poll_fd = {
		.fd = fd,
		.events = POLLIN,
	};
	int ret;

	do {
		ret = poll(&poll_fd, 1, timeout_ms);
	} while (ret < 0 && errno == EINTR);
	return ret;
}

static void run_restart_waiter(uint64_t dev, uint32_t event_nr, int ready_fd,
			       int signal_fd, int result_fd)
{
	struct dm_wait_result result = { 0 };
	struct sigaction action;
	char ready = 1;
	int fd;

	fd = open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC);
	if (fd < 0)
		_exit(EXIT_FAILURE);
	memset(&action, 0, sizeof(action));
	action.sa_handler = restart_signal_handler;
	action.sa_flags = SA_RESTART;
	if (sigemptyset(&action.sa_mask) < 0 ||
	    sigaction(SIGUSR1, &action, NULL) < 0)
		_exit(EXIT_FAILURE);
	restart_signal_fd = signal_fd;

	init_ioctl(&result.header);
	result.header.dev = dev;
	result.header.event_nr = event_nr;
	if (write(ready_fd, &ready, sizeof(ready)) != sizeof(ready))
		_exit(EXIT_FAILURE);
	if (ioctl(fd, DM_DEV_WAIT, &result.header) < 0)
		result.error = errno;
	close(fd);
	if (write(result_fd, &result, sizeof(result)) != sizeof(result))
		_exit(EXIT_FAILURE);
	close(ready_fd);
	close(signal_fd);
	close(result_fd);
	_exit(result.error == 0 ? EXIT_SUCCESS : EXIT_FAILURE);
}

// Verifies the ABI envelope accepted by the control device and the LVM reload
// suppression sequence: create a tableless device, inspect its active table,
// then remove it. The table-status query must succeed with target_count == 0.
FN_TEST(device_mapper_reports_version_before_parameter_errors)
{
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	for (int i = 0; i < 3; i++) {
		struct dm_ioctl io;

		init_ioctl(&io);
		io.version[1] = 0;
		io.version[2] = UINT32_MAX;
		if (i == 0)
			io.data_size = 0;
		else if (i == 1)
			io.flags = 1U << 31;
		else
			memset(io.name, 'x', sizeof(io.name));

		/* Parameter rejection must not hide the earlier ABI handshake. */
		TEST(ioctl(fd, DM_DEV_STATUS, &io), i == 0 ? EINVAL : ENXIO,
		     _ret == -1);
		TEST_RES(io.version[0] == DM_VERSION_MAJOR, _ret == 1);
		TEST_RES(io.version[1] == DM_VERSION_MINOR, _ret == 1);
		TEST_RES(io.version[2] == DM_VERSION_PATCHLEVEL, _ret == 1);
	}
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_persistent_minor_error_precedes_identity_collision)
{
	char name[DM_NAME_LEN], other[DM_NAME_LEN], uuid[DM_UUID_LEN];
	struct dm_ioctl io;
	uint64_t dev;
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	snprintf(name, sizeof(name), "dm-create-order-%ld", (long)getpid());
	snprintf(other, sizeof(other), "dm-create-other-%ld", (long)getpid());
	snprintf(uuid, sizeof(uuid), "dm-create-uuid-%ld", (long)getpid());
	init_named_ioctl(&io, name);
	snprintf(io.uuid, sizeof(io.uuid), "%s", uuid);
	if (TEST_RES(ioctl(fd, DM_DEV_CREATE, &io), _ret == 0) < 0)
		goto close_control;
	dev = io.dev;
	for (int collision = 0; collision < 3; collision++) {
		init_named_ioctl(&io, collision == 1 ? name : other);
		if (collision == 2)
			snprintf(io.uuid, sizeof(io.uuid), "%s", uuid);
		io.flags = DM_PERSISTENT_DEV_FLAG;
		io.dev = dev;
		TEST(ioctl(fd, DM_DEV_CREATE, &io), ENXIO, _ret == -1);
	}
	/* Without a requested minor, identity collisions still return EEXIST. */
	init_named_ioctl(&io, name);
	TEST(ioctl(fd, DM_DEV_CREATE, &io), EEXIST, _ret == -1);
	init_named_ioctl(&io, other);
	snprintf(io.uuid, sizeof(io.uuid), "%s", uuid);
	TEST(ioctl(fd, DM_DEV_CREATE, &io), EEXIST, _ret == -1);
	init_named_ioctl(&io, name);
	TEST_RES(ioctl(fd, DM_DEV_STATUS, &io), _ret == 0);
	TEST_RES(io.dev, _ret == dev);
	TEST_RES(memcmp(io.uuid, uuid, strlen(uuid) + 1), _ret == 0);
	init_named_ioctl(&io, other);
	TEST(ioctl(fd, DM_DEV_STATUS, &io), ENXIO, _ret == -1);
	init_ioctl(&io);
	io.dev = dev;
	TEST_RES(ioctl(fd, DM_DEV_REMOVE, &io), _ret == 0);
close_control:
	TEST_RES(close(fd), _ret == 0);
}
END_TEST()

FN_TEST(device_mapper_tableless_status_is_linux_compatible)
{
	struct dm_ioctl io;
	int fd;
	uint64_t dev;

	TEST_RES(sizeof(struct dm_ioctl), _ret == DM_IOCTL_ENVELOPE_SIZE);
	fd = TEST_SUCC(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	init_ioctl(&io);
	TEST_SUCC(ioctl(fd, DM_VERSION, &io));
	TEST_RES(io.version[0] == DM_VERSION_MAJOR, _ret == 1);
	TEST_RES(io.version[1] == DM_VERSION_MINOR, _ret == 1);
	TEST_RES(io.version[2] == DM_VERSION_PATCHLEVEL, _ret == 1);
	TEST_RES(io.data_size == DM_IOCTL_ENVELOPE_SIZE, _ret == 1);
	TEST_RES(io.data_start == DM_IOCTL_ENVELOPE_SIZE, _ret == 1);

	init_ioctl(&io);
	snprintf(io.name, sizeof(io.name), "dm-abi-%ld", (long)getpid());
	TEST_SUCC(ioctl(fd, DM_DEV_CREATE, &io));
	TEST_RES(io.flags & (1U << 2), _ret == 0);
	TEST_RES(!(io.flags & DM_SUSPEND_FLAG), _ret == 1);
	TEST_RES(io.target_count == 0, _ret == 1);
	dev = io.dev;

	init_ioctl(&io);
	io.dev = dev;
	TEST_SUCC(ioctl(fd, DM_DEV_STATUS, &io));
	TEST_RES(io.flags & (1U << 2), _ret == 0);
	TEST_RES(!(io.flags & DM_SUSPEND_FLAG), _ret == 1);
	TEST_RES(io.target_count == 0, _ret == 1);

	init_ioctl(&io);
	io.dev = dev;
	TEST_SUCC(ioctl(fd, DM_TABLE_STATUS, &io));
	TEST_RES(io.flags & (1U << 2), _ret == 0);
	TEST_RES(io.target_count == 0, _ret == 1);

	init_ioctl(&io);
	io.dev = dev;
	TEST_SUCC(ioctl(fd, DM_DEV_REMOVE, &io));
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_runtime_nodes_rollback_on_path_collision)
{
	char name[DM_NAME_LEN];
	char primary[64];
	char alias[DM_NAME_LEN + 16];
	struct dm_ioctl io;
	struct stat primary_stat;
	struct stat alias_stat;
	union dm_table_buffer table;
	uint64_t dev;
	int fd;
	int collision_fd;

	snprintf(name, sizeof(name), "dm-node-rollback-%ld", (long)getpid());
	fd = TEST_SUCC(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_CREATE, &io));
	dev = io.dev;
	mapper_paths(dev, name, primary, sizeof(primary), alias, sizeof(alias));

	collision_fd =
		TEST_SUCC(open(primary, O_CREAT | O_EXCL | O_WRONLY, 0600));
	TEST_SUCC(close(collision_fd));
	init_linear_table(table.bytes, sizeof(table.bytes), name, 8);
	TEST_ERRNO(ioctl(fd, DM_TABLE_LOAD, table.bytes), EEXIST);
	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_STATUS, &io));
	TEST_RES(io.flags & DM_INACTIVE_PRESENT_FLAG, _ret == 0);
	TEST_ERRNO(access(alias, F_OK), ENOENT);

	TEST_SUCC(unlink(primary));
	init_linear_table(table.bytes, sizeof(table.bytes), name, 8);
	TEST_SUCC(ioctl(fd, DM_TABLE_LOAD, table.bytes));
	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_STATUS, &io));
	TEST_RES(io.flags & DM_INACTIVE_PRESENT_FLAG, _ret != 0);

	collision_fd =
		TEST_SUCC(open(alias, O_CREAT | O_EXCL | O_WRONLY, 0600));
	TEST_SUCC(close(collision_fd));
	init_named_ioctl(&io, name);
	TEST_ERRNO(ioctl(fd, DM_DEV_SUSPEND, &io), EEXIST);
	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_STATUS, &io));
	TEST_RES(io.flags & DM_INACTIVE_PRESENT_FLAG, _ret != 0);
	TEST_RES(io.flags & DM_ACTIVE_PRESENT_FLAG, _ret == 0);

	TEST_SUCC(unlink(alias));
	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_SUSPEND, &io));
	TEST_SUCC(lstat(alias, &alias_stat));
	TEST_RES(S_ISLNK(alias_stat.st_mode), _ret != 0);
	TEST_SUCC(stat(primary, &primary_stat));
	TEST_SUCC(stat(alias, &alias_stat));
	TEST_RES(alias_stat.st_rdev == primary_stat.st_rdev, _ret != 0);

	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_REMOVE, &io));
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_mount_lease_blocks_remove_until_umount)
{
	char name[DM_NAME_LEN];
	char primary[64];
	char alias[DM_NAME_LEN + 16];
	char mount_dir[64];
	struct dm_ioctl io;
	union dm_table_buffer table;
	int fd;

	snprintf(name, sizeof(name), "dm-mount-lease-%ld", (long)getpid());
	snprintf(mount_dir, sizeof(mount_dir), "/tmp/dm-mount-%ld",
		 (long)getpid());
	TEST_SUCC(unshare(CLONE_NEWNS));
	TEST_SUCC(mkdir(mount_dir, 0755));
	fd = TEST_SUCC(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_CREATE, &io));
	mapper_paths(io.dev, name, primary, sizeof(primary), alias,
		     sizeof(alias));
	init_linear_table(table.bytes, sizeof(table.bytes), name, 1024 * 1024);
	TEST_SUCC(ioctl(fd, DM_TABLE_LOAD, table.bytes));
	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_SUSPEND, &io));

	TEST_SUCC(mount(alias, mount_dir, "ext2", MS_RDONLY, NULL));
	init_named_ioctl(&io, name);
	TEST_ERRNO(ioctl(fd, DM_DEV_REMOVE, &io), EBUSY);
	TEST_SUCC(umount(mount_dir));
	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_REMOVE, &io));
	TEST_SUCC(close(fd));
	TEST_SUCC(rmdir(mount_dir));
}
END_TEST()

FN_TEST(device_mapper_range_ioctl_validates_byte_ranges)
{
	static const unsigned long commands[] = { BLKDISCARD, BLKZEROOUT };
	static const struct {
		uint64_t start;
		uint64_t length;
		int error;
	} cases[] = {
		{ 0, 4096, 0 },
		{ 512, 512, 0 },
		{ 3584, 512, 0 },
		{ 1, 512, EINVAL },
		{ 0, 1, EINVAL },
		{ 0, 513, EINVAL },
		{ UINT64_MAX - UINT64_C(511), 512, EINVAL },
		{ 4096, 512, EINVAL },
		{ 3584, 1024, EINVAL },
		{ 0, 0, 0 },
		{ 512, 0, 0 },
		{ 4096, 0, 0 },
		{ 1, 0, EINVAL },
		{ 4608, 0, EINVAL },
	};
	char name[DM_NAME_LEN];
	char primary[64];
	char alias[DM_NAME_LEN + 16];
	struct dm_ioctl io;
	union dm_table_buffer table;
	uint64_t dev = 0;
	uint64_t device_size = 0;
	int control_fd = -1;
	int mapper_fd = -1;
	int device_created = 0;

	snprintf(name, sizeof(name), "dm-range-%ld", (long)getpid());
	control_fd =
		TEST_RES(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC), _ret >= 0);
	if (control_fd < 0)
		goto cleanup;
	init_named_ioctl(&io, name);
	if (TEST_RES(ioctl(control_fd, DM_DEV_CREATE, &io), _ret == 0) < 0)
		goto cleanup;
	dev = io.dev;
	device_created = 1;

	init_zero_table(table.bytes, sizeof(table.bytes), name, 8);
	if (TEST_RES(ioctl(control_fd, DM_TABLE_LOAD, table.bytes), _ret == 0) <
	    0)
		goto cleanup;
	init_named_ioctl(&io, name);
	if (TEST_RES(ioctl(control_fd, DM_DEV_SUSPEND, &io), _ret == 0) < 0)
		goto cleanup;
	mapper_paths(dev, name, primary, sizeof(primary), alias, sizeof(alias));
	mapper_fd = TEST_RES(open(alias, O_RDWR | O_CLOEXEC), _ret >= 0);
	if (mapper_fd < 0)
		goto cleanup;
	if (TEST_RES(ioctl(mapper_fd, BLKGETSIZE64, &device_size),
		     _ret == 0 && device_size == 4096) < 0)
		goto cleanup;

	for (size_t command = 0;
	     command < sizeof(commands) / sizeof(commands[0]); command++) {
		for (size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); i++) {
			uint64_t range[2] = { cases[i].start, cases[i].length };
			int expected_result = cases[i].error == 0 ? 0 : -1;

			TEST(ioctl(mapper_fd, commands[command], range),
			     cases[i].error, _ret == expected_result);
		}
	}

cleanup:
	if (mapper_fd >= 0)
		TEST_RES(close(mapper_fd), _ret == 0);
	if (device_created) {
		init_ioctl(&io);
		io.dev = dev;
		TEST_RES(ioctl(control_fd, DM_DEV_REMOVE, &io), _ret == 0);
	}
	if (control_fd >= 0)
		TEST_RES(close(control_fd), _ret == 0);
}
END_TEST()

FN_TEST(device_mapper_wait_reports_eintr_and_missing_device)
{
	char name[DM_NAME_LEN];
	struct dm_ioctl io;
	struct sigaction action;
	struct sigaction old_action;
	uint32_t event_nr;
	int fd;

	snprintf(name, sizeof(name), "dm-wait-%ld", (long)getpid());
	fd = TEST_SUCC(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));
	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_CREATE, &io));

	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_STATUS, &io));
	event_nr = io.event_nr;
	memset(&action, 0, sizeof(action));
	action.sa_handler = alarm_handler;
	TEST_SUCC(sigemptyset(&action.sa_mask));
	TEST_SUCC(sigaction(SIGALRM, &action, &old_action));
	TEST_RES(alarm(1), _ret == 0);
	init_named_ioctl(&io, name);
	io.event_nr = event_nr;
	TEST_ERRNO(ioctl(fd, DM_DEV_WAIT, &io), EINTR);
	TEST_RES(alarm(0), _ret == 0);
	TEST_SUCC(sigaction(SIGALRM, &old_action, NULL));

	init_named_ioctl(&io, "dm-wait-missing");
	TEST_ERRNO(ioctl(fd, DM_DEV_WAIT, &io), ENXIO);
	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_REMOVE, &io));
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_wait_restarts_after_signal)
{
	char name[DM_NAME_LEN];
	char renamed[DM_NAME_LEN];
	char notification;
	int ready_pipe[2] = { -1, -1 };
	int signal_pipe[2] = { -1, -1 };
	int result_pipe[2] = { -1, -1 };
	int status;
	int fd = -1;
	int wait_result;
	pid_t pid = -1;
	uint64_t dev = 0;
	struct dm_ioctl io;
	struct dm_wait_result result = { 0 };
	union dm_table_buffer rename;
	union dm_table_buffer table;
	uint32_t event_nr;
	int child_reaped = 0;
	int device_created = 0;

	snprintf(name, sizeof(name), "dm-wait-restart-%ld", (long)getpid());
	snprintf(renamed, sizeof(renamed), "dm-wait-restarted-%ld",
		 (long)getpid());
	fd = TEST_RES(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC), _ret >= 0);
	if (fd < 0)
		goto cleanup;
	init_named_ioctl(&io, name);
	if (TEST_RES(ioctl(fd, DM_DEV_CREATE, &io), _ret == 0) < 0)
		goto cleanup;
	dev = io.dev;
	device_created = 1;
	/* Rename publishes a device event only while a live table exists. */
	init_zero_table(table.bytes, sizeof(table.bytes), name, 8);
	if (TEST_RES(ioctl(fd, DM_TABLE_LOAD, table.bytes), _ret == 0) < 0)
		goto cleanup;
	init_named_ioctl(&io, name);
	if (TEST_RES(ioctl(fd, DM_DEV_SUSPEND, &io), _ret == 0) < 0)
		goto cleanup;
	event_nr = io.event_nr;

	if (TEST_RES(pipe(ready_pipe), _ret == 0) < 0 ||
	    TEST_RES(pipe(signal_pipe), _ret == 0) < 0 ||
	    TEST_RES(pipe(result_pipe), _ret == 0) < 0)
		goto cleanup;
	pid = TEST_RES(fork(), _ret >= 0);
	if (pid == 0) {
		close(ready_pipe[0]);
		close(signal_pipe[0]);
		close(result_pipe[0]);
		run_restart_waiter(dev, event_nr, ready_pipe[1], signal_pipe[1],
				   result_pipe[1]);
	}
	if (pid < 0)
		goto cleanup;
	close(ready_pipe[1]);
	ready_pipe[1] = -1;
	close(signal_pipe[1]);
	signal_pipe[1] = -1;
	close(result_pipe[1]);
	result_pipe[1] = -1;

	wait_result =
		TEST_RES(wait_fd_readable(ready_pipe[0], 2000), _ret == 1);
	if (wait_result != 1)
		goto cleanup;
	if (TEST_RES(read(ready_pipe[0], &notification, sizeof(notification)),
		     _ret == sizeof(notification)) != sizeof(notification))
		goto cleanup;

	/* The original ioctl must still be pending when the signal is sent. */
	wait_result =
		TEST_RES(wait_fd_readable(result_pipe[0], 100), _ret == 0);
	if (wait_result != 0)
		goto cleanup;
	if (TEST_RES(kill(pid, SIGUSR1), _ret == 0) < 0)
		goto cleanup;
	wait_result =
		TEST_RES(wait_fd_readable(signal_pipe[0], 2000), _ret == 1);
	if (wait_result != 1)
		goto cleanup;
	if (TEST_RES(read(signal_pipe[0], &notification, sizeof(notification)),
		     _ret == sizeof(notification)) != sizeof(notification))
		goto cleanup;

	/* SA_RESTART must keep the original ioctl pending until a DM event occurs. */
	wait_result =
		TEST_RES(wait_fd_readable(result_pipe[0], 100), _ret == 0);
	if (wait_result != 0)
		goto cleanup;
	init_rename_ioctl(rename.bytes, sizeof(rename.bytes), name, 0, renamed);
	if (TEST_RES(ioctl(fd, DM_DEV_RENAME, rename.bytes), _ret == 0) < 0)
		goto cleanup;
	wait_result =
		TEST_RES(wait_fd_readable(result_pipe[0], 2000), _ret == 1);
	if (wait_result != 1)
		goto cleanup;
	if (TEST_RES(read(result_pipe[0], &result, sizeof(result)),
		     _ret == sizeof(result)) != sizeof(result))
		goto cleanup;
	if (TEST_RES(waitpid(pid, &status, 0),
		     _ret == pid && WIFEXITED(status) &&
			     WEXITSTATUS(status) == EXIT_SUCCESS) == pid)
		child_reaped = 1;
	TEST_RES(result.error, _ret == 0);
	TEST_RES(result.header.event_nr, _ret == event_nr + 1);
	TEST_RES(strcmp(result.header.name, renamed), _ret == 0);

cleanup:
	if (pid > 0 && !child_reaped) {
		kill(pid, SIGKILL);
		waitpid(pid, NULL, 0);
	}
	if (ready_pipe[0] >= 0)
		close(ready_pipe[0]);
	if (ready_pipe[1] >= 0)
		close(ready_pipe[1]);
	if (signal_pipe[0] >= 0)
		close(signal_pipe[0]);
	if (signal_pipe[1] >= 0)
		close(signal_pipe[1]);
	if (result_pipe[0] >= 0)
		close(result_pipe[0]);
	if (result_pipe[1] >= 0)
		close(result_pipe[1]);
	if (device_created) {
		init_ioctl(&io);
		io.dev = dev;
		TEST_RES(ioctl(fd, DM_DEV_REMOVE, &io), _ret == 0);
	}
	if (fd >= 0)
		TEST_RES(close(fd), _ret == 0);
}
END_TEST()

FN_TEST(device_mapper_wait_wakes_after_rename_and_setuuid)
{
	char name[DM_NAME_LEN];
	char renamed[DM_NAME_LEN];
	char uuid[DM_UUID_LEN];
	char ready;
	int ready_pipe[2] = { -1, -1 };
	int result_pipe[2] = { -1, -1 };
	int status;
	int fd = -1;
	pid_t pid = -1;
	uint64_t dev = 0;
	uint32_t event_nr;
	int device_created = 0;
	struct dm_ioctl io;
	struct dm_wait_result result = { 0 };
	union dm_table_buffer rename;
	union dm_table_buffer table;

	snprintf(name, sizeof(name), "dm-wait-wake-%ld", (long)getpid());
	snprintf(renamed, sizeof(renamed), "dm-wait-woke-%ld", (long)getpid());
	snprintf(uuid, sizeof(uuid), "dm-wait-uuid-%ld", (long)getpid());
	fd = TEST_RES(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC), _ret >= 0);
	if (fd < 0)
		goto cleanup;
	init_named_ioctl(&io, name);
	if (TEST_RES(ioctl(fd, DM_DEV_CREATE, &io), _ret == 0) < 0)
		goto cleanup;
	dev = io.dev;
	device_created = 1;
	init_zero_table(table.bytes, sizeof(table.bytes), name, 8);
	if (TEST_RES(ioctl(fd, DM_TABLE_LOAD, table.bytes), _ret == 0) < 0)
		goto cleanup;
	init_named_ioctl(&io, name);
	if (TEST_RES(ioctl(fd, DM_DEV_SUSPEND, &io), _ret == 0) < 0)
		goto cleanup;
	event_nr = io.event_nr;

	for (int set_uuid = 0; set_uuid < 2; set_uuid++) {
		pid = TEST_RES(spawn_waiter(dev, event_nr + set_uuid,
					    ready_pipe, result_pipe),
			       _ret >= 0);
		if (pid < 0)
			goto cleanup;
		if (TEST_RES(wait_fd_readable(ready_pipe[0], 2000),
			     _ret == 1) != 1)
			goto cleanup;
		if (TEST_RES(read(ready_pipe[0], &ready, sizeof(ready)),
			     _ret == sizeof(ready)) != sizeof(ready))
			goto cleanup;
		/* Pending output does not prove that the kernel waiter is registered. */
		if (TEST_RES(wait_fd_readable(result_pipe[0], 100),
			     _ret == 0) != 0)
			goto cleanup;
		init_rename_ioctl(rename.bytes, sizeof(rename.bytes),
				  set_uuid ? renamed : name,
				  set_uuid ? DM_UUID_FLAG : 0,
				  set_uuid ? uuid : renamed);
		if (TEST_RES(ioctl(fd, DM_DEV_RENAME, rename.bytes),
			     _ret == 0) < 0)
			goto cleanup;
		if (TEST_RES(wait_fd_readable(result_pipe[0], 2000),
			     _ret == 1) != 1)
			goto cleanup;
		if (TEST_RES(read(result_pipe[0], &result, sizeof(result)),
			     _ret == sizeof(result)) != sizeof(result))
			goto cleanup;
		if (TEST_RES(waitpid(pid, &status, 0),
			     _ret == pid && WIFEXITED(status) &&
				     WEXITSTATUS(status) == EXIT_SUCCESS) !=
		    pid)
			goto cleanup;
		pid = -1;
		TEST_RES(close(ready_pipe[0]), _ret == 0);
		ready_pipe[0] = -1;
		TEST_RES(close(result_pipe[0]), _ret == 0);
		result_pipe[0] = -1;
		TEST_RES(result.error, _ret == 0);
		TEST_RES(result.header.event_nr,
			 _ret == event_nr + set_uuid + 1);
		TEST_RES(strcmp(result.header.name, renamed), _ret == 0);
		TEST_RES(strcmp(result.header.uuid, set_uuid ? uuid : ""),
			 _ret == 0);
	}

cleanup:
	if (pid > 0) {
		kill(pid, SIGKILL);
		waitpid(pid, NULL, 0);
	}
	for (int i = 0; i < 2; i++) {
		if (ready_pipe[i] >= 0)
			close(ready_pipe[i]);
		if (result_pipe[i] >= 0)
			close(result_pipe[i]);
	}
	if (device_created) {
		init_ioctl(&io);
		io.dev = dev;
		TEST_RES(ioctl(fd, DM_DEV_REMOVE, &io), _ret == 0);
	}
	if (fd >= 0)
		TEST_RES(close(fd), _ret == 0);
}
END_TEST()

FN_TEST(device_mapper_fixed_prefix_and_no_params_guard_pages)
{
	const size_t sizes[] = { 305, 311, 312, 1024 * 1024 };
	long page_size = CHECK_WITH(sysconf(_SC_PAGESIZE), _ret > 0);
	unsigned char *pages =
		CHECK_WITH(mmap(NULL, page_size * 2, PROT_READ | PROT_WRITE,
				MAP_PRIVATE | MAP_ANONYMOUS, -1, 0),
			   _ret != MAP_FAILED);
	char name[DM_NAME_LEN];
	struct dm_ioctl io;
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	TEST_RES(offsetof(struct dm_ioctl, data), _ret == DM_IOCTL_FIXED_SIZE);
	snprintf(name, sizeof(name), "dm-prefix-%ld", (long)getpid());
	CHECK(mprotect(pages + page_size, page_size, PROT_NONE));
	init_named_ioctl(&io, name);
	CHECK(ioctl(fd, DM_DEV_CREATE, &io));
	for (size_t i = 0; i < sizeof(sizes) / sizeof(sizes[0]); i++) {
		size_t accessible = sizes[i] > 312 ? 305 : sizes[i];
		unsigned char *arg = pages + page_size - accessible;

		for (int secure = 0; secure < 2; secure++) {
			init_named_ioctl(&io, name);
			io.data_size = sizes[i];
			io.data_start = UINT32_MAX;
			io.flags = secure ? DM_SECURE_DATA_FLAG : 0;
			memset(arg, 0xa5, accessible);
			memcpy(arg, &io, DM_IOCTL_FIXED_SIZE);
			/* Direction/size bits do not change the command's copy contract. */
			TEST_RES(ioctl(fd, _IO(0xfd, DM_DEV_STATUS_CMD), arg),
				 _ret == 0);
			TEST_RES(raw_u32(arg + 12),
				 _ret == DM_IOCTL_FIXED_SIZE);
			TEST_RES(raw_u32(arg + 16), _ret == UINT32_MAX);
			TEST_RES(raw_u32(arg + 28) & DM_SECURE_DATA_FLAG,
				 _ret == 0);
			TEST_RES(bytes_are(arg + 305, accessible - 305, 0xa5),
				 _ret == 1);
		}
	}
	init_named_ioctl(&io, name);
	TEST_RES(ioctl(fd, DM_DEV_REMOVE, &io), _ret == 0);
	TEST_SUCC(munmap(pages, page_size * 2));
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_failure_copyout_and_secure_clear_boundaries)
{
	union dm_table_buffer input, before;
	char name[DM_NAME_LEN];
	struct dm_ioctl io;
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	snprintf(name, sizeof(name), "dm-secure-%ld", (long)getpid());
	init_named_ioctl(&io, name);
	CHECK(ioctl(fd, DM_DEV_CREATE, &io));
	for (int secure = 0; secure < 2; secure++) {
		memset(input.bytes, 0xa5, sizeof(input.bytes));
		init_named_ioctl(&input.align, name);
		input.align.version[1] = 0;
		input.align.version[2] = UINT32_MAX;
		input.align.data_size = sizeof(input.bytes);
		input.align.flags = secure ? DM_SECURE_DATA_FLAG : 0;
		/* An empty load is rejected after copying and clearing the request. */
		memcpy(&before, &input, sizeof(before));
		TEST(ioctl(fd, DM_TABLE_LOAD, input.bytes), EINVAL, _ret == -1);
		if (secure) {
			TEST_RES(bytes_are(input.bytes, sizeof(input.bytes), 0),
				 _ret == 1);
		} else {
			TEST_RES(input.align.version[1],
				 _ret == DM_VERSION_MINOR);
			TEST_RES(memcmp(input.bytes + 12, before.bytes + 12,
					sizeof(input.bytes) - 12),
				 _ret == 0);
		}
	}

	/* Secure success returns only the header, not the copied table payload. */
	init_zero_table(input.bytes, sizeof(input.bytes), name, 8);
	input.align.data_size = sizeof(input.bytes);
	input.align.flags = DM_SECURE_DATA_FLAG;
	TEST_RES(ioctl(fd, DM_TABLE_LOAD, input.bytes), _ret == 0);
	TEST_RES(input.align.data_size, _ret == 305);
	TEST_RES(bytes_are(input.bytes + 305, sizeof(input.bytes) - 305, 0),
		 _ret == 1);
	TEST_RES(input.align.flags & DM_SECURE_DATA_FLAG, _ret == 0);
	init_named_ioctl(&io, name);
	TEST_RES(ioctl(fd, DM_DEV_REMOVE, &io), _ret == 0);
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_secure_fault_prevents_dispatch)
{
	long page_size = CHECK_WITH(sysconf(_SC_PAGESIZE), _ret > 0);
	unsigned char *pages =
		CHECK_WITH(mmap(NULL, page_size * 2, PROT_READ | PROT_WRITE,
				MAP_PRIVATE | MAP_ANONYMOUS, -1, 0),
			   _ret != MAP_FAILED);
	unsigned char *arg;
	struct dm_ioctl io;
	unsigned char before[DM_IOCTL_FIXED_SIZE];
	char name[DM_NAME_LEN];
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	snprintf(name, sizeof(name), "dm-clear-fault-%ld", (long)getpid());
	/* Version and the fixed prefix are writable, but payload copyin faults. */
	arg = pages + page_size - 312;
	init_named_ioctl(&io, name);
	io.data_size = 384;
	io.flags = DM_SECURE_DATA_FLAG;
	memcpy(arg, &io, sizeof(io));
	memcpy(before, arg, sizeof(before));
	CHECK(mprotect(pages + page_size, page_size, PROT_NONE));
	TEST(ioctl(fd, DM_TABLE_LOAD, arg), EFAULT, _ret == -1);
	TEST_RES(memcmp(arg + 12, before + 12, sizeof(before) - 12), _ret == 0);

	/* A readable but read-only header tail fails secure clear before create. */
	CHECK(mprotect(pages + page_size, page_size, PROT_READ | PROT_WRITE));
	arg = pages + page_size - 64;
	init_named_ioctl(&io, name);
	io.flags = DM_SECURE_DATA_FLAG;
	memcpy(arg, &io, sizeof(io));
	memcpy(before, arg, sizeof(before));
	CHECK(mprotect(pages + page_size, page_size, PROT_READ));
	TEST(ioctl(fd, DM_DEV_CREATE, arg), EFAULT, _ret == -1);
	TEST_RES(memcmp(arg + 64, before + 64, sizeof(before) - 64), _ret == 0);
	init_named_ioctl(&io, name);
	TEST(ioctl(fd, DM_DEV_STATUS, &io), ENXIO, _ret == -1);
	TEST_SUCC(munmap(pages, page_size * 2));
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_selectors_and_query_flags)
{
	char name[DM_NAME_LEN], uuid[DM_UUID_LEN];
	struct dm_ioctl io;
	uint64_t dev;
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));
	const uint32_t preserved = DM_SKIP_BDGET_FLAG | DM_SKIP_LOCKFS_FLAG |
				   DM_NOFLUSH_FLAG | DM_STATUS_TABLE_FLAG |
				   (1U << 31);
	const uint32_t cleared = DM_BUFFER_FULL_FLAG |
				 DM_UEVENT_GENERATED_FLAG |
				 DM_SECURE_DATA_FLAG | DM_DATA_OUT_FLAG |
				 DM_ACTIVE_PRESENT_FLAG |
				 DM_INACTIVE_PRESENT_FLAG | DM_SUSPEND_FLAG;

	snprintf(name, sizeof(name), "dm-select-%ld", (long)getpid());
	snprintf(uuid, sizeof(uuid), "dm-select-uuid-%ld", (long)getpid());
	init_named_ioctl(&io, name);
	snprintf(io.uuid, sizeof(io.uuid), "%s", uuid);
	CHECK(ioctl(fd, DM_DEV_CREATE, &io));
	dev = io.dev;
	for (unsigned mask = 0; mask < 8; mask++) {
		int expected = mask == 3 || mask == 7 ? EINVAL : ENXIO;
		init_ioctl(&io);
		if (mask & 1)
			snprintf(io.name, sizeof(io.name), "%s", name);
		if (mask & 2)
			snprintf(io.uuid, sizeof(io.uuid), "%s", uuid);
		if (mask & 4)
			io.dev = dev;
		if (mask == 1 || mask == 2 || mask == 4) {
			TEST_RES(ioctl(fd, DM_DEV_STATUS, &io), _ret == 0);
			TEST_RES(io.dev, _ret == dev);
		} else {
			TEST(ioctl(fd, DM_DEV_STATUS, &io), expected,
			     _ret == -1);
		}
	}
	init_named_ioctl(&io, name);
	io.flags = preserved | cleared;
	TEST_RES(ioctl(fd, DM_DEV_STATUS, &io), _ret == 0);
	TEST_RES(io.flags & preserved, _ret == preserved);
	TEST_RES(io.flags & cleared, _ret == 0);
	TEST_RES(io.flags & (1U << 2), _ret == 0);
	init_named_ioctl(&io, name);
	TEST_RES(ioctl(fd, DM_DEV_REMOVE, &io), _ret == 0);
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_name_list_raw_layout_and_empty_copyout)
{
	char name[DM_NAME_LEN], uuid[DM_UUID_LEN];
	struct dm_ioctl io;
	union dm_table_buffer output;
	uint64_t dev;
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));
	size_t extension, needed;

	snprintf(name, sizeof(name), "dm-list-%ld", (long)getpid());
	snprintf(uuid, sizeof(uuid), "dm-list-uuid-%ld", (long)getpid());
	init_named_ioctl(&io, name);
	snprintf(io.uuid, sizeof(io.uuid), "%s", uuid);
	CHECK(ioctl(fd, DM_DEV_CREATE, &io));
	dev = io.dev;
	extension = align_to_u64(12 + strlen(name) + 1);
	for (int with_uuid = 0; with_uuid < 2; with_uuid++) {
		needed = align_to_u64(extension + 8 +
				      (with_uuid ? strlen(uuid) + 1 : 0));
		memset(output.bytes, 0xa5, sizeof(output.bytes));
		init_named_ioctl(&output.align, name);
		snprintf(output.align.uuid, sizeof(output.align.uuid), "%s",
			 uuid);
		output.align.data_size = sizeof(output.bytes);
		output.align.flags = with_uuid ? DM_UUID_FLAG : 0;
		TEST_RES(ioctl(fd, DM_LIST_DEVICES, output.bytes), _ret == 0);
		TEST_RES(output.align.data_start, _ret == 312);
		TEST_RES(output.align.data_size, _ret == 312 + needed);
		TEST_RES(raw_u64(output.bytes + 312), _ret == dev);
		TEST_RES(raw_u32(output.bytes + 320), _ret == 0);
		TEST_RES(memcmp(output.bytes + 324, name, strlen(name) + 1),
			 _ret == 0);
		TEST_RES(raw_u32(output.bytes + 312 + extension), _ret == 0);
		TEST_RES(raw_u32(output.bytes + 316 + extension),
			 _ret == (unsigned)with_uuid);
		if (with_uuid)
			TEST_RES(memcmp(output.bytes + 320 + extension, uuid,
					strlen(uuid) + 1),
				 _ret == 0);
		TEST_RES(bytes_are(output.bytes + 312 + needed,
				   sizeof(output.bytes) - 312 - needed, 0xa5),
			 _ret == 1);

		memset(output.bytes, 0xa5, sizeof(output.bytes));
		init_named_ioctl(&output.align, name);
		output.align.flags = with_uuid ? DM_UUID_FLAG : 0;
		output.align.data_size = 312 + needed - 1;
		TEST_RES(ioctl(fd, DM_LIST_DEVICES, output.bytes), _ret == 0);
		TEST_RES(output.align.flags & DM_BUFFER_FULL_FLAG, _ret != 0);
		TEST_RES(output.align.data_size, _ret == 305);
		TEST_RES(bytes_are(output.bytes + 312,
				   sizeof(output.bytes) - 312, 0xa5),
			 _ret == 1);
	}

	memset(output.bytes, 0xa5, sizeof(output.bytes));
	init_named_ioctl(&output.align, name);
	snprintf(output.align.uuid, sizeof(output.align.uuid), "no-such-uuid");
	output.align.data_size = sizeof(output.bytes);
	TEST_RES(ioctl(fd, DM_LIST_DEVICES, output.bytes), _ret == 0);
	TEST_RES(output.align.data_size, _ret == 312);
	/* Linux prepares a sentinel internally but does not include it in copyout. */
	TEST_RES(bytes_are(output.bytes + 312, sizeof(output.bytes) - 312,
			   0xa5),
		 _ret == 1);
	init_named_ioctl(&io, name);
	TEST_RES(ioctl(fd, DM_DEV_REMOVE, &io), _ret == 0);
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_wait_returns_table_records)
{
	char name[DM_NAME_LEN];
	struct dm_ioctl io;
	union dm_table_buffer table;
	uint32_t event_nr;
	uint64_t dev;
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	snprintf(name, sizeof(name), "dm-wait-records-%ld", (long)getpid());
	init_named_ioctl(&io, name);
	if (TEST_RES(ioctl(fd, DM_DEV_CREATE, &io), _ret == 0) < 0)
		goto close_control;
	dev = io.dev;
	init_zero_table(table.bytes, sizeof(table.bytes), name, 8);
	if (TEST_RES(ioctl(fd, DM_TABLE_LOAD, table.bytes), _ret == 0) < 0)
		goto cleanup;
	init_named_ioctl(&io, name);
	if (TEST_RES(ioctl(fd, DM_DEV_SUSPEND, &io), _ret == 0) < 0)
		goto cleanup;
	event_nr = io.event_nr;
	init_zero_table(table.bytes, sizeof(table.bytes), name, 16);
	table.align.flags = DM_READONLY_FLAG;
	if (TEST_RES(ioctl(fd, DM_TABLE_LOAD, table.bytes), _ret == 0) < 0)
		goto cleanup;
	for (int inactive = 0; inactive < 2; inactive++) {
		for (int full = 0; full < 2; full++) {
			uint32_t flags =
				DM_STATUS_TABLE_FLAG | DM_NOFLUSH_FLAG |
				(inactive ? DM_QUERY_INACTIVE_TABLE_FLAG : 0);
			memset(table.bytes, 0xa5, sizeof(table.bytes));
			init_named_ioctl(&table.align, name);
			table.align.flags = flags;
			table.align.event_nr = event_nr - 1;
			table.align.data_size = full ? sizeof(table.bytes) :
						       312;
			TEST_RES(ioctl(fd, DM_DEV_WAIT, table.bytes),
				 _ret == 0);
			TEST_RES(table.align.target_count, _ret == 1);
			TEST_RES(table.align.event_nr, _ret == event_nr);
			TEST_RES(table.align.flags & flags, _ret == flags);
			TEST_RES(!!(table.align.flags & DM_READONLY_FLAG),
				 _ret == inactive);
			if (full) {
				TEST_RES(table.align.data_size, _ret == 353);
				TEST_RES(raw_u64(table.bytes + 320),
					 _ret == (inactive ? 16 : 8));
				TEST_RES(raw_u32(table.bytes + 332),
					 _ret == 48);
				TEST_RES(memcmp(table.bytes + 336, "zero",
						sizeof("zero")),
					 _ret == 0);
				TEST_RES(table.bytes[352], _ret == 0);
				TEST_RES(table.bytes[353], _ret == 0xa5);
			} else {
				TEST_RES(table.align.flags &
						 DM_BUFFER_FULL_FLAG,
					 _ret != 0);
				TEST_RES(table.align.data_size, _ret == 305);
			}
		}
	}
cleanup:
	init_ioctl(&io);
	io.dev = dev;
	TEST_RES(ioctl(fd, DM_DEV_REMOVE, &io), _ret == 0);
close_control:
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_relative_and_escaped_backing_paths)
{
	char name[DM_NAME_LEN], link_name[64], escaped[80];
	union dm_table_buffer table;
	struct dm_ioctl io;
	int cwd = -1, fd = -1;
	int link_created = 0, device_created = 0;
	uint64_t dev = 0;

	snprintf(name, sizeof(name), "dm-path-%ld", (long)getpid());
	snprintf(link_name, sizeof(link_name), "dm backing-%ld",
		 (long)getpid());
	snprintf(escaped, sizeof(escaped), "dm\\ backing-%ld 0",
		 (long)getpid());
	cwd = TEST_RES(open(".", O_RDONLY | O_DIRECTORY | O_CLOEXEC),
		       _ret >= 0);
	if (cwd < 0)
		goto cleanup;
	fd = TEST_RES(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC), _ret >= 0);
	if (fd < 0)
		goto cleanup;
	if (TEST_RES(chdir("/dev"), _ret == 0) < 0)
		goto cleanup;
	if (TEST_RES(symlink("vdc", link_name), _ret == 0) < 0)
		goto cleanup;
	link_created = 1;
	init_named_ioctl(&io, name);
	if (TEST_RES(ioctl(fd, DM_DEV_CREATE, &io), _ret == 0) < 0)
		goto cleanup;
	dev = io.dev;
	device_created = 1;
	for (int quote = 0; quote < 2; quote++) {
		init_linear_table(table.bytes, sizeof(table.bytes), name, 8);
		char *params = (char *)table.bytes + 312 + 40;
		snprintf(params, sizeof(table.bytes) - 352, "%s",
			 quote ? escaped : "vdc 0");
		table.align.data_size =
			312 + align_to_u64(40 + strlen(params) + 1);
		TEST_RES(ioctl(fd, DM_TABLE_LOAD, table.bytes), _ret == 0);
	}
cleanup:
	if (device_created) {
		init_ioctl(&io);
		io.dev = dev;
		TEST_RES(ioctl(fd, DM_DEV_REMOVE, &io), _ret == 0);
	}
	if (link_created)
		TEST_RES(unlink(link_name), _ret == 0);
	if (cwd >= 0) {
		/* Continuing in /dev would change the meaning of later relative paths. */
		CHECK(fchdir(cwd));
		TEST_RES(close(cwd), _ret == 0);
	}
	if (fd >= 0)
		TEST_RES(close(fd), _ret == 0);
}
END_TEST()

FN_TEST(device_mapper_rejected_identity_changes_preserve_state)
{
	char name[DM_NAME_LEN], uuid[DM_UUID_LEN];
	char primary[64], alias[DM_NAME_LEN + 32], too_long[DM_UUID_LEN + 1];
	struct dm_ioctl io;
	union dm_table_buffer input;
	struct stat before, after;
	uint64_t dev;
	uint32_t event_nr;
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	snprintf(name, sizeof(name), "dm-identity-%ld", (long)getpid());
	snprintf(uuid, sizeof(uuid), "dm-identity-uuid-%ld", (long)getpid());
	init_named_ioctl(&io, name);
	if (TEST_RES(ioctl(fd, DM_DEV_CREATE, &io), _ret == 0) < 0)
		goto close_control;
	dev = io.dev;
	init_zero_table(input.bytes, sizeof(input.bytes), name, 8);
	if (TEST_RES(ioctl(fd, DM_TABLE_LOAD, input.bytes), _ret == 0) < 0)
		goto cleanup;
	init_named_ioctl(&io, name);
	if (TEST_RES(ioctl(fd, DM_DEV_SUSPEND, &io), _ret == 0) < 0)
		goto cleanup;
	mapper_paths(dev, name, primary, sizeof(primary), alias, sizeof(alias));
	if (TEST_RES(stat(alias, &before), _ret == 0) < 0)
		goto cleanup;
	init_rename_ioctl(input.bytes, sizeof(input.bytes), name, DM_UUID_FLAG,
			  uuid);
	if (TEST_RES(ioctl(fd, DM_DEV_RENAME, input.bytes), _ret == 0) < 0)
		goto cleanup;
	event_nr = input.align.event_nr;
	memset(too_long, 'x', sizeof(too_long) - 1);
	too_long[sizeof(too_long) - 1] = '\0';
	const struct {
		uint32_t flags;
		const char *value;
		int error;
	} cases[] = {
		{ 0, "bad/name", EINVAL },
		{ 0, too_long, EINVAL },
		{ 0, name, EBUSY },
		{ DM_UUID_FLAG, "", EINVAL },
		{ DM_UUID_FLAG, too_long, EINVAL },
		{ DM_UUID_FLAG, uuid, EBUSY },
		{ DM_UUID_FLAG, "dm-another-uuid", EINVAL },
	};
	for (size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); i++) {
		init_rename_ioctl(input.bytes, sizeof(input.bytes), name,
				  cases[i].flags, cases[i].value);
		TEST(ioctl(fd, DM_DEV_RENAME, input.bytes), cases[i].error,
		     _ret == -1);
		init_named_ioctl(&io, name);
		TEST_RES(ioctl(fd, DM_DEV_STATUS, &io), _ret == 0);
		TEST_RES(io.dev, _ret == dev);
		TEST_RES(io.event_nr, _ret == event_nr);
		TEST_RES(strcmp(io.uuid, uuid), _ret == 0);
		init_ioctl(&io);
		snprintf(io.uuid, sizeof(io.uuid), "%s", uuid);
		TEST_RES(ioctl(fd, DM_DEV_STATUS, &io), _ret == 0);
		TEST_RES(io.dev, _ret == dev);
		if (TEST_RES(stat(alias, &after), _ret == 0) == 0) {
			TEST_RES(after.st_rdev, _ret == before.st_rdev);
			TEST_RES(after.st_ino, _ret == before.st_ino);
		}
	}
	/* Creation rejects names containing a path separator as well. */
	init_named_ioctl(&io, "bad/name");
	TEST(ioctl(fd, DM_DEV_CREATE, &io), EINVAL, _ret == -1);
cleanup:
	init_ioctl(&io);
	io.dev = dev;
	TEST_RES(ioctl(fd, DM_DEV_REMOVE, &io), _ret == 0);
close_control:
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_readonly_table_mode_transitions)
{
	char name[DM_NAME_LEN];
	struct dm_ioctl io;
	union dm_table_buffer table;
	uint64_t dev;
	int fd;

	snprintf(name, sizeof(name), "dm-readonly-mode-%ld", (long)getpid());
	fd = TEST_SUCC(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));
	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_CREATE, &io));
	dev = io.dev;

	init_zero_table(table.bytes, sizeof(table.bytes), name, 8);
	table.align.flags = DM_READONLY_FLAG;
	TEST_SUCC(ioctl(fd, DM_TABLE_LOAD, table.bytes));
	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_STATUS, &io));
	TEST_RES(io.flags & DM_INACTIVE_PRESENT_FLAG, _ret != 0);
	TEST_RES(!(io.flags & DM_ACTIVE_PRESENT_FLAG), _ret == 1);
	TEST_RES(!(io.flags & DM_READONLY_FLAG), _ret == 1);

	init_named_ioctl(&io, name);
	io.flags = DM_QUERY_INACTIVE_TABLE_FLAG;
	TEST_SUCC(ioctl(fd, DM_DEV_STATUS, &io));
	TEST_RES(io.flags & DM_READONLY_FLAG, _ret != 0);

	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_SUSPEND, &io));
	TEST_RES(io.flags & DM_ACTIVE_PRESENT_FLAG, _ret != 0);
	TEST_RES(io.flags & DM_READONLY_FLAG, _ret != 0);

	init_zero_table(table.bytes, sizeof(table.bytes), name, 8);
	TEST_SUCC(ioctl(fd, DM_TABLE_LOAD, table.bytes));
	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_STATUS, &io));
	TEST_RES(io.flags & DM_READONLY_FLAG, _ret != 0);
	TEST_RES(io.flags & DM_INACTIVE_PRESENT_FLAG, _ret != 0);

	init_named_ioctl(&io, name);
	io.flags = DM_QUERY_INACTIVE_TABLE_FLAG;
	TEST_SUCC(ioctl(fd, DM_DEV_STATUS, &io));
	TEST_RES(!(io.flags & DM_READONLY_FLAG), _ret == 1);

	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_SUSPEND, &io));
	TEST_RES(io.flags & DM_ACTIVE_PRESENT_FLAG, _ret != 0);
	TEST_RES(!(io.flags & DM_READONLY_FLAG), _ret == 1);

	init_ioctl(&io);
	io.dev = dev;
	TEST_SUCC(ioctl(fd, DM_DEV_REMOVE, &io));
	TEST_SUCC(close(fd));
}
END_TEST()
