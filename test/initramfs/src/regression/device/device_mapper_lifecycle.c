// SPDX-License-Identifier: MPL-2.0

/* Device identity, node rollback, mount leases, and event-wait lifecycle.
 * The linear mount scenario requires the ext2 test filesystem on /dev/vdc. */
#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <sys/ioctl.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#include "../common/test.h"
#include "device_mapper_common.h"

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
		io.dev = dev | (UINT64_C(1) << 32);
		TEST(ioctl(fd, DM_DEV_CREATE, &io), ENXIO, _ret == -1);
	}
	/* Linux dm_hash_insert reports EBUSY for either identity collision. */
	init_named_ioctl(&io, name);
	TEST(ioctl(fd, DM_DEV_CREATE, &io), EBUSY, _ret == -1);
	init_named_ioctl(&io, other);
	snprintf(io.uuid, sizeof(io.uuid), "%s", uuid);
	TEST(ioctl(fd, DM_DEV_CREATE, &io), EBUSY, _ret == -1);
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
