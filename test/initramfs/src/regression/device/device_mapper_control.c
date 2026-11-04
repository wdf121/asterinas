// SPDX-License-Identifier: MPL-2.0

/* VERSION, tableless CREATE, and inactive linear LOAD ABI regression tests. */

#include <fcntl.h>
#include <stdio.h>
#include <linux/dm-ioctl.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <unistd.h>

#include "../common/test.h"
#include "device_mapper_common.h"

#define DM_CONTROL_PATH "/dev/mapper/control"

static const uint32_t kernel_version[3] = { 4, 48, 0 };

FN_TEST(device_mapper_control_node)
{
	struct stat st;
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	TEST_RES(fstat(fd, &st), S_ISCHR(st.st_mode) &&
					 major(st.st_rdev) == 10 &&
					 minor(st.st_rdev) == 236 &&
					 (st.st_mode & 0777) == 0600);
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_version_compatibility)
{
	const struct {
		uint32_t version[3];
		int error;
	} cases[] = {
		{ { 4, 0, 0 }, 0 },	      { { 4, 48, 0 }, 0 },
		{ { 4, 48, UINT32_MAX }, 0 }, { { 3, 0, 0 }, EINVAL },
		{ { 5, 0, 0 }, EINVAL },      { { 4, 49, 0 }, EINVAL },
	};
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	for (size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); i++) {
		struct dm_ioctl io = {};

		memcpy(io.version, cases[i].version, sizeof(io.version));
		TEST(ioctl(fd, DM_VERSION, &io), cases[i].error,
		     _ret == (cases[i].error ? -1 : 0));
		TEST_RES(memcmp(io.version, kernel_version, sizeof(io.version)),
			 _ret == 0);
	}
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_version_precedes_command_lookup)
{
	struct dm_ioctl io = {};
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	io.version[0] = 4;
	TEST(ioctl(fd, _IOWR(0xfd, 0xff, struct dm_ioctl), &io), ENOTTY,
	     _ret == -1);
	TEST_RES(memcmp(io.version, kernel_version, sizeof(io.version)),
		 _ret == 0);

	io.version[0] = 5;
	TEST(ioctl(fd, _IOWR(0xfd, 0xff, struct dm_ioctl), &io), EINVAL,
	     _ret == -1);
	TEST_RES(memcmp(io.version, kernel_version, sizeof(io.version)),
		 _ret == 0);
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
	uint32_t *version =
		(uint32_t *)(pages + page_size - sizeof(kernel_version));
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	CHECK(mprotect(pages + page_size, page_size, PROT_NONE));
	const unsigned long commands[] = {
		DM_VERSION,
		_IO(0xfd, DM_VERSION_CMD),
		_IOC(_IOC_READ, 0xfd, DM_VERSION_CMD, 1),
	};
	for (size_t i = 0; i < sizeof(commands) / sizeof(commands[0]); i++) {
		version[0] = 4;
		version[1] = 0;
		version[2] = 0;
		TEST_RES(ioctl(fd, commands[i], version), _ret == 0);
		TEST_RES(memcmp(version, kernel_version,
				sizeof(kernel_version)),
			 _ret == 0);
	}
	TEST_SUCC(munmap(pages, page_size * 2));
	TEST_SUCC(close(fd));
}
END_TEST()

FN_TEST(device_mapper_create_tableless)
{
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));
	struct dm_ioctl io = {};
	char name[DM_NAME_LEN];
	char uuid[DM_UUID_LEN];
	char collision_name[DM_NAME_LEN];
	char retry_name[DM_NAME_LEN];

	snprintf(name, sizeof(name), "dm-phase1-%ld", (long)getpid());
	snprintf(uuid, sizeof(uuid), "dm-phase1-uuid-%ld", (long)getpid());
	snprintf(collision_name, sizeof(collision_name),
		 "dm-phase1-uuid-collision-%ld", (long)getpid());
	snprintf(retry_name, sizeof(retry_name), "dm-phase1-minor-retry-%ld",
		 (long)getpid());
	memcpy(io.version, kernel_version, sizeof(io.version));
	io.data_size = sizeof(io);
	strcpy(io.name, name);
	strcpy(io.uuid, uuid);
	TEST_RES(ioctl(fd, DM_DEV_CREATE, &io), _ret == 0);
	TEST_RES(strcmp(io.name, name), _ret == 0);
	TEST_RES(strcmp(io.uuid, uuid), _ret == 0);
	TEST_RES(io.dev, _ret != 0 && minor(io.dev) == 0);
	TEST_RES(io.data_size, _ret == offsetof(struct dm_ioctl, data));
	TEST_RES(io.target_count, _ret == 0);
	TEST_RES(io.flags, _ret == 0);

	struct dm_ioctl duplicate = {};
	memcpy(duplicate.version, kernel_version, sizeof(duplicate.version));
	duplicate.data_size = sizeof(duplicate);
	strcpy(duplicate.name, name);
	TEST(ioctl(fd, DM_DEV_CREATE, &duplicate), EBUSY, _ret == -1);

	/* Number allocation precedes name/UUID collision checks. */
	duplicate.flags = DM_PERSISTENT_DEV_FLAG;
	duplicate.dev = io.dev;
	TEST(ioctl(fd, DM_DEV_CREATE, &duplicate), ENXIO, _ret == -1);

	struct dm_ioctl duplicate_uuid = {};
	memcpy(duplicate_uuid.version, kernel_version,
	       sizeof(duplicate_uuid.version));
	duplicate_uuid.data_size = sizeof(duplicate_uuid);
	strcpy(duplicate_uuid.name, collision_name);
	strcpy(duplicate_uuid.uuid, uuid);
	duplicate_uuid.flags = DM_PERSISTENT_DEV_FLAG;
	duplicate_uuid.dev = makedev(major(io.dev), 7);
	TEST(ioctl(fd, DM_DEV_CREATE, &duplicate_uuid), EBUSY, _ret == -1);

	struct dm_ioctl retry = {};
	memcpy(retry.version, kernel_version, sizeof(retry.version));
	retry.data_size = sizeof(retry);
	strcpy(retry.name, retry_name);
	retry.flags = DM_PERSISTENT_DEV_FLAG;
	retry.dev = duplicate_uuid.dev;
	TEST_RES(ioctl(fd, DM_DEV_CREATE, &retry),
		 _ret == 0 && minor(retry.dev) == 7);

	struct dm_ioctl invalid = {};
	memcpy(invalid.version, kernel_version, sizeof(invalid.version));
	invalid.data_size = sizeof(invalid);
	strcpy(invalid.name, "invalid/name");
	TEST(ioctl(fd, DM_DEV_CREATE, &invalid), EINVAL, _ret == -1);

	TEST_SUCC(close(fd));
}
END_TEST()

/* No REMOVE command yet: the mapper and backing lease remain until reboot.
 * Run with the official fresh /dev/vdd regression disk present.
 */
FN_TEST(device_mapper_first_linear_load_publishes_primary)
{
	char name[DM_NAME_LEN];
	char primary[64];
	struct dm_ioctl io = {};
	struct stat st;
	union dm_table_buffer table;
	dev_t dev;
	int fd = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	snprintf(name, sizeof(name), "dm-load-%ld", (long)getpid());
	memcpy(io.version, kernel_version, sizeof(io.version));
	io.data_size = sizeof(io);
	strcpy(io.name, name);
	TEST_SUCC(ioctl(fd, DM_DEV_CREATE, &io));
	dev = (dev_t)io.dev;
	snprintf(primary, sizeof(primary), "/dev/dm-%u", minor(dev));
	TEST(stat(primary, &st), ENOENT, _ret == -1);

	init_linear_table(table.bytes, sizeof(table.bytes), name, 8);
	table.align.target_count = 0;
	TEST(ioctl(fd, DM_TABLE_LOAD, table.bytes), EINVAL, _ret == -1);
	TEST(stat(primary, &st), ENOENT, _ret == -1);

	init_linear_table(table.bytes, sizeof(table.bytes), name, 8);
	TEST_SUCC(ioctl(fd, DM_TABLE_LOAD, table.bytes));
	TEST_RES(table.align.dev, _ret == (uint64_t)dev);
	TEST_RES(table.align.data_size,
		 _ret == offsetof(struct dm_ioctl, data));
	TEST_RES(table.align.target_count, _ret == 1);
	TEST_RES(table.align.flags & DM_INACTIVE_PRESENT_FLAG, _ret != 0);
	TEST_RES(table.align.flags & DM_ACTIVE_PRESENT_FLAG, _ret == 0);
	TEST_RES(stat(primary, &st),
		 _ret == 0 && S_ISBLK(st.st_mode) && st.st_rdev == dev);
	TEST_SUCC(close(fd));
}
END_TEST()

/* LOAD-only coverage: this test must also work before RESUME is introduced. */
FN_TEST(device_mapper_numeric_backing_load)
{
	char name[DM_NAME_LEN], primary[64], backing_id[32];
	struct stat backing, node;
	struct dm_ioctl create = {};
	union dm_table_buffer table;
	int control = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));
	CHECK(stat("/dev/vdd", &backing));
	snprintf(backing_id, sizeof(backing_id), "%u:%u",
		 major(backing.st_rdev), minor(backing.st_rdev));
	snprintf(name, sizeof(name), "dm-numeric-load-%ld", (long)getpid());
	memcpy(create.version, kernel_version, sizeof(create.version));
	create.data_size = sizeof(create);
	strcpy(create.name, name);
	TEST_RES(ioctl(control, DM_DEV_CREATE, &create), _ret == 0);
	snprintf(primary, sizeof(primary), "/dev/dm-%u", minor(create.dev));
	init_linear_table_params(table.bytes, sizeof(table.bytes), name, 8,
				 backing_id, 160);
	TEST_RES(ioctl(control, DM_TABLE_LOAD, table.bytes), _ret == 0);
	TEST_RES(table.align.target_count, _ret == 1);
	TEST_RES(table.align.flags & DM_INACTIVE_PRESENT_FLAG, _ret != 0);
	TEST_RES(table.align.flags & DM_ACTIVE_PRESENT_FLAG, _ret == 0);
	TEST_RES(stat(primary, &node), _ret == 0 && S_ISBLK(node.st_mode) &&
					       node.st_rdev == create.dev);
	TEST_SUCC(close(control));
}
END_TEST()
