// SPDX-License-Identifier: MPL-2.0

/* Control-device ABI: version negotiation, capability checks, request
 * boundaries, selectors, and copyout. These tests use raw ioctls, not dmsetup. */
#define _GNU_SOURCE

#include <fcntl.h>
#include <poll.h>
#include <stddef.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#include "../common/capability.h"
#include "../common/test.h"
#include "device_mapper_common.h"

static int bytes_are(const unsigned char *bytes, size_t len,
		     unsigned char value)
{
	for (size_t i = 0; i < len; i++) {
		if (bytes[i] != value)
			return 0;
	}
	return 1;
}

/* This implementation advertises the ABI shared by Linux 6.18.56 and 7.2.10. */
static const uint32_t kernel_version[3] = { 4, 50, 0 };
static const uint32_t client_version[3] = { 4, 0, UINT32_MAX };

FN_TEST(device_mapper_control_without_events_is_not_ready)
{
	const short masks[] = { POLLIN, POLLOUT, POLLIN | POLLOUT };
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	for (size_t i = 0; i < sizeof(masks) / sizeof(masks[0]); i++) {
		struct pollfd poll_fd = {
			.fd = fd,
			.events = masks[i],
			.revents = -1,
		};

		TEST_RES(poll(&poll_fd, 1, 0), _ret == 0);
		TEST_RES(poll_fd.revents, _ret == 0);
	}
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_version_compatibility)
{
	const struct {
		uint32_t version[3];
		int error;
	} cases[] = {
		{ { 4, 0, 0 }, 0 },
		{ { 4, 49, 0 }, 0 },
		{ { 4, 50, UINT32_MAX }, 0 },
		{ { 3, 0, 0 }, EINVAL },
		{ { 5, 0, 0 }, EINVAL },
		{ { 4, 51, 0 }, EINVAL },
		{ { 4, UINT32_MAX, 0 }, EINVAL },
	};
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	for (size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); i++) {
		struct dm_ioctl io = {};

		memcpy(io.version, cases[i].version, sizeof(io.version));
		io.data_size = sizeof(io);
		io.data_start = sizeof(io);
		TEST(ioctl(fd, DM_VERSION, &io), cases[i].error,
		     _ret == (cases[i].error ? -1 : 0));
		TEST_RES(memcmp(io.version, kernel_version, sizeof(io.version)),
			 _ret == 0);
	}
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_negotiates_before_command_lookup)
{
	struct dm_ioctl io = {}, before;
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	memcpy(io.version, client_version, sizeof(io.version));
	memcpy(&before, &io, sizeof(io));
	TEST(ioctl(fd, _IOWR(0xfe, 0, struct dm_ioctl), &io), ENOTTY,
	     _ret == -1);
	TEST_RES(memcmp(&io, &before, sizeof(io)), _ret == 0);

	/* An unknown command in the DM namespace still exchanges versions. */
	TEST(ioctl(fd, _IOWR(0xfd, 0xff, struct dm_ioctl), &io), ENOTTY,
	     _ret == -1);
	TEST_RES(memcmp(io.version, kernel_version, sizeof(io.version)),
		 _ret == 0);

	memcpy(io.version, client_version, sizeof(io.version));
	io.version[0] = 5;
	TEST(ioctl(fd, _IOWR(0xfd, 0xff, struct dm_ioctl), &io), EINVAL,
	     _ret == -1);
	TEST_RES(memcmp(io.version, kernel_version, sizeof(io.version)),
		 _ret == 0);
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_version_ignores_non_version_fields)
{
	const uint32_t sizes[] = { 0, 312, 1024 * 1024 + 1, UINT32_MAX };
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	for (size_t i = 0; i < sizeof(sizes) / sizeof(sizes[0]); i++) {
		struct dm_ioctl io, before;

		/* Include unsupported flags and a name without a terminating NUL. */
		memset(&io, 0xff, sizeof(io));
		memcpy(io.version, client_version, sizeof(io.version));
		io.data_size = sizes[i];
		io.data_start = i;
		memcpy(&before, &io, sizeof(io));
		TEST_RES(ioctl(fd, DM_VERSION, &io), _ret == 0);
		TEST_RES(memcmp(io.version, kernel_version, sizeof(io.version)),
			 _ret == 0);
		TEST_RES(memcmp((char *)&io + sizeof(io.version),
				(char *)&before + sizeof(io.version),
				sizeof(io) - sizeof(io.version)),
			 _ret == 0);
	}
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_version_only_accesses_version_bytes)
{
	long page_size = CHECK_WITH(sysconf(_SC_PAGESIZE), _ret > 0);
	char *pages =
		CHECK_WITH(mmap(NULL, page_size * 2, PROT_READ | PROT_WRITE,
				MAP_PRIVATE | MAP_ANONYMOUS, -1, 0),
			   _ret != MAP_FAILED);
	char *version = pages + page_size - sizeof(kernel_version);
	struct dm_ioctl io = {};
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	memcpy(io.version, client_version, sizeof(io.version));
	io.data_size = sizeof(io);
	io.data_start = sizeof(io);
	memcpy(version, &io, sizeof(io));

	/* No read beyond version[3] is needed, despite the ioctl size encoding. */
	CHECK(mprotect(pages + page_size, page_size, PROT_NONE));
	const unsigned long commands[] = {
		DM_VERSION,
		_IO(0xfd, DM_VERSION_CMD),
		_IOC(_IOC_READ, 0xfd, DM_VERSION_CMD, 1),
		_IOC(_IOC_WRITE, 0xfd, DM_VERSION_CMD, 0x3fff),
	};
	for (size_t i = 0; i < sizeof(commands) / sizeof(commands[0]); i++) {
		memcpy(version, client_version, sizeof(client_version));
		TEST_RES(ioctl(fd, commands[i], version), _ret == 0);
		TEST_RES(memcmp(version, kernel_version,
				sizeof(kernel_version)),
			 _ret == 0);
	}

	/* Nor may a successful handshake write the readable, read-only tail. */
	CHECK(mprotect(pages + page_size, page_size, PROT_READ));
	memcpy(version, client_version, sizeof(client_version));
	TEST_RES(ioctl(fd, DM_VERSION, version), _ret == 0);
	TEST_RES(memcmp(version, kernel_version, sizeof(kernel_version)),
		 _ret == 0);
	TEST_RES(memcmp(pages + page_size, (char *)&io + sizeof(io.version),
			sizeof(io) - sizeof(io.version)),
		 _ret == 0);

	TEST_SUCC(munmap(pages, page_size * 2));
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_version_reports_memory_faults)
{
	long page_size = CHECK_WITH(sysconf(_SC_PAGESIZE), _ret > 0);
	struct dm_ioctl *io =
		CHECK_WITH(mmap(NULL, page_size, PROT_READ | PROT_WRITE,
				MAP_PRIVATE | MAP_ANONYMOUS, -1, 0),
			   _ret != MAP_FAILED);
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	io->data_size = sizeof(*io);
	io->data_start = sizeof(*io);
	memcpy(io->version, client_version, sizeof(io->version));
	CHECK(mprotect(io, page_size, PROT_READ));
	TEST(ioctl(fd, DM_VERSION, io), EFAULT, _ret == -1);

	/* A failed version copyout takes precedence over an incompatible major. */
	CHECK(mprotect(io, page_size, PROT_READ | PROT_WRITE));
	io->version[0] = 5;
	CHECK(mprotect(io, page_size, PROT_READ));
	TEST(ioctl(fd, DM_VERSION, io), EFAULT, _ret == -1);

	CHECK(mprotect(io, page_size, PROT_NONE));
	TEST(ioctl(fd, DM_VERSION, io), EFAULT, _ret == -1);
	TEST_SUCC(munmap(io, page_size));
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_requires_admin_on_each_ioctl)
{
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));
	pid_t pid = CHECK(fork());
	int status;

	if (pid == 0) {
		struct dm_ioctl io = {}, before;
		int result, error;

		/* Retain the open fd and root UID, but remove CAP_SYS_ADMIN. */
		CHECK_WITH(geteuid(), _ret == 0);
		drop_capability(CAP_SYS_ADMIN);
		memcpy(io.version, client_version, sizeof(io.version));
		io.data_size = sizeof(io);
		io.data_start = sizeof(io);
		memcpy(&before, &io, sizeof(io));
		errno = 0;
		result = ioctl(fd, DM_VERSION, &io);
		error = errno;
		if (result != -1 || error != EACCES ||
		    memcmp(&io, &before, sizeof(io)) != 0)
			_exit(EXIT_FAILURE);

		/* Reject mutating commands before even accessing their arguments. */
		errno = 0;
		result = ioctl(fd, DM_DEV_CREATE, (void *)UINTPTR_MAX);
		error = errno;
		close(fd);
		_exit(result == -1 && error == EACCES ? EXIT_SUCCESS :
							EXIT_FAILURE);
	}
	TEST_RES(waitpid(pid, &status, 0),
		 _ret == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
	TEST_SUCC(close(fd));
}
END_TEST()

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
