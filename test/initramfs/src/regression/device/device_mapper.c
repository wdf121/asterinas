// SPDX-License-Identifier: MPL-2.0

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
#include <sched.h>
#include <sys/ioctl.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <sys/wait.h>
#include <unistd.h>

#include "../common/test.h"

#define DM_CONTROL_PATH "/dev/mapper/control"
#define DM_IOCTL_ENVELOPE_SIZE 312
#define DM_TABLE_BUFFER_SIZE 512
/* DM_EXISTS_FLAG is a kernel-private ABI status bit, absent from Linux UAPI. */
#define DM_EXISTS_FLAG (1U << 2)

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
			      uint64_t dev, uint32_t flags, const char *value)
{
	struct dm_ioctl *io = (struct dm_ioctl *)buffer;
	char *data;

	memset(buffer, 0, buffer_size);
	init_ioctl(io);
	io->dev = dev;
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
	close(result_pipe[1]);
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
		write_result = write(restart_signal_fd, &delivered,
				     sizeof(delivered));
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
	TEST_RES(io.flags & DM_EXISTS_FLAG, _ret != 0);
	TEST_RES(!(io.flags & DM_SUSPEND_FLAG), _ret == 1);
	TEST_RES(io.target_count == 0, _ret == 1);
	dev = io.dev;

	init_ioctl(&io);
	io.dev = dev;
	TEST_SUCC(ioctl(fd, DM_DEV_STATUS, &io));
	TEST_RES(io.flags & DM_EXISTS_FLAG, _ret != 0);
	TEST_RES(!(io.flags & DM_SUSPEND_FLAG), _ret == 1);
	TEST_RES(io.target_count == 0, _ret == 1);

	init_ioctl(&io);
	io.dev = dev;
	TEST_SUCC(ioctl(fd, DM_TABLE_STATUS, &io));
	TEST_RES(io.flags & DM_EXISTS_FLAG, _ret != 0);
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
	control_fd = TEST_RES(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC),
			      _ret >= 0);
	if (control_fd < 0)
		goto cleanup;
	init_named_ioctl(&io, name);
	if (TEST_RES(ioctl(control_fd, DM_DEV_CREATE, &io), _ret == 0) < 0)
		goto cleanup;
	dev = io.dev;
	device_created = 1;

	init_zero_table(table.bytes, sizeof(table.bytes), name, 8);
	if (TEST_RES(ioctl(control_fd, DM_TABLE_LOAD, table.bytes), _ret == 0) < 0)
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

	if (TEST_RES(pipe(ready_pipe), _ret == 0) < 0 ||
	    TEST_RES(pipe(signal_pipe), _ret == 0) < 0 ||
	    TEST_RES(pipe(result_pipe), _ret == 0) < 0)
		goto cleanup;
	pid = TEST_RES(fork(), _ret >= 0);
	if (pid == 0) {
		close(ready_pipe[0]);
		close(signal_pipe[0]);
		close(result_pipe[0]);
		run_restart_waiter(dev, 0, ready_pipe[1], signal_pipe[1],
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

	wait_result = TEST_RES(wait_fd_readable(ready_pipe[0], 2000),
			       _ret == 1);
	if (wait_result != 1)
		goto cleanup;
	if (TEST_RES(read(ready_pipe[0], &notification, sizeof(notification)),
		     _ret == sizeof(notification)) != sizeof(notification))
		goto cleanup;

	/* The original ioctl must still be pending when the signal is sent. */
	wait_result = TEST_RES(wait_fd_readable(result_pipe[0], 100), _ret == 0);
	if (wait_result != 0)
		goto cleanup;
	if (TEST_RES(kill(pid, SIGUSR1), _ret == 0) < 0)
		goto cleanup;
	wait_result = TEST_RES(wait_fd_readable(signal_pipe[0], 2000),
			       _ret == 1);
	if (wait_result != 1)
		goto cleanup;
	if (TEST_RES(read(signal_pipe[0], &notification, sizeof(notification)),
		     _ret == sizeof(notification)) != sizeof(notification))
		goto cleanup;

	/* SA_RESTART must keep the original ioctl pending until a DM event occurs. */
	wait_result = TEST_RES(wait_fd_readable(result_pipe[0], 100), _ret == 0);
	if (wait_result != 0)
		goto cleanup;
	init_rename_ioctl(rename.bytes, sizeof(rename.bytes), dev, 0, renamed);
	if (TEST_RES(ioctl(fd, DM_DEV_RENAME, rename.bytes), _ret == 0) < 0)
		goto cleanup;
	wait_result = TEST_RES(wait_fd_readable(result_pipe[0], 2000),
			       _ret == 1);
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
	TEST_RES(result.header.event_nr, _ret == 1);
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
	int ready_pipe[2];
	int result_pipe[2];
	int status;
	int fd;
	pid_t pid;
	uint64_t dev;
	struct dm_ioctl io;
	struct dm_wait_result result;
	union dm_table_buffer rename;

	snprintf(name, sizeof(name), "dm-wait-wake-%ld", (long)getpid());
	snprintf(renamed, sizeof(renamed), "dm-wait-woke-%ld", (long)getpid());
	snprintf(uuid, sizeof(uuid), "dm-wait-uuid-%ld", (long)getpid());
	fd = TEST_SUCC(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));
	init_named_ioctl(&io, name);
	TEST_SUCC(ioctl(fd, DM_DEV_CREATE, &io));
	dev = io.dev;

	pid = TEST_SUCC(spawn_waiter(dev, 0, ready_pipe, result_pipe));
	TEST_RES(read(ready_pipe[0], &ready, sizeof(ready)),
		 _ret == sizeof(ready));
	TEST_SUCC(sched_yield());
	init_rename_ioctl(rename.bytes, sizeof(rename.bytes), dev, 0, renamed);
	TEST_SUCC(ioctl(fd, DM_DEV_RENAME, rename.bytes));
	TEST_RES(read(result_pipe[0], &result, sizeof(result)),
		 _ret == sizeof(result));
	TEST_SUCC(close(ready_pipe[0]));
	TEST_SUCC(close(result_pipe[0]));
	TEST_RES(waitpid(pid, &status, 0),
		 _ret == pid && WIFEXITED(status) &&
			 WEXITSTATUS(status) == EXIT_SUCCESS);
	TEST_RES(result.error, _ret == 0);
	TEST_RES(result.header.event_nr, _ret == 1);
	TEST_RES(strcmp(result.header.name, renamed), _ret == 0);
	TEST_RES(strcmp(result.header.uuid, ""), _ret == 0);

	pid = TEST_SUCC(spawn_waiter(dev, 1, ready_pipe, result_pipe));
	TEST_RES(read(ready_pipe[0], &ready, sizeof(ready)),
		 _ret == sizeof(ready));
	TEST_SUCC(sched_yield());
	init_rename_ioctl(rename.bytes, sizeof(rename.bytes), dev, DM_UUID_FLAG,
			  uuid);
	TEST_SUCC(ioctl(fd, DM_DEV_RENAME, rename.bytes));
	TEST_RES(read(result_pipe[0], &result, sizeof(result)),
		 _ret == sizeof(result));
	TEST_SUCC(close(ready_pipe[0]));
	TEST_SUCC(close(result_pipe[0]));
	TEST_RES(waitpid(pid, &status, 0),
		 _ret == pid && WIFEXITED(status) &&
			 WEXITSTATUS(status) == EXIT_SUCCESS);
	TEST_RES(result.error, _ret == 0);
	TEST_RES(result.header.event_nr, _ret == 2);
	TEST_RES(strcmp(result.header.name, renamed), _ret == 0);
	TEST_RES(strcmp(result.header.uuid, uuid), _ret == 0);

	init_named_ioctl(&io, renamed);
	TEST_SUCC(ioctl(fd, DM_DEV_REMOVE, &io));
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

	init_named_ioctl(&io, name);
	io.dev = dev;
	TEST_SUCC(ioctl(fd, DM_DEV_REMOVE, &io));
	TEST_SUCC(close(fd));
}
END_TEST()
