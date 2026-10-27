// SPDX-License-Identifier: MPL-2.0

/* Mapping-table queries, backing-path parsing, read-only transitions, and
 * mapped-device range ioctls. Linear path tests require /dev/vdc. */
#define _GNU_SOURCE

#include <fcntl.h>
#include <linux/fs.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include "../common/test.h"
#include "device_mapper_common.h"

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
	TEST_RES(memcmp(io.version, kernel_version, sizeof(io.version)),
		 _ret == 0);
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
