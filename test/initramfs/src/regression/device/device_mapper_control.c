// SPDX-License-Identifier: MPL-2.0

/* VERSION, tableless CREATE, linear LOAD and first RESUME ABI regression tests. */

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

#define TEST_PROGRAM_SUMMARY
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
	TEST_RES(io.dev, _ret != 0);
	TEST_RES(io.data_size, _ret == offsetof(struct dm_ioctl, data));
	TEST_RES(io.target_count, _ret == 0);
	TEST_RES(io.flags, _ret == 0);

	struct dm_ioctl duplicate = {};
	memcpy(duplicate.version, kernel_version, sizeof(duplicate.version));
	duplicate.data_size = sizeof(duplicate);
	strcpy(duplicate.name, name);
	TEST(ioctl(fd, DM_DEV_CREATE, &duplicate), EBUSY, _ret == -1);

	/* A reserved minor cannot be claimed by another mapper. */
	duplicate.flags = DM_PERSISTENT_DEV_FLAG;
	duplicate.dev = io.dev;
	TEST(ioctl(fd, DM_DEV_CREATE, &duplicate), ENXIO, _ret == -1);

	struct dm_ioctl duplicate_uuid = {};
	memcpy(duplicate_uuid.version, kernel_version,
	       sizeof(duplicate_uuid.version));
	duplicate_uuid.data_size = sizeof(duplicate_uuid);
	strcpy(duplicate_uuid.name, collision_name);
	strcpy(duplicate_uuid.uuid, uuid);
	TEST(ioctl(fd, DM_DEV_CREATE, &duplicate_uuid), EBUSY, _ret == -1);

	/* Do not assume the next free minor is adjacent to io.dev. */
	struct dm_ioctl retry = {};
	memcpy(retry.version, kernel_version, sizeof(retry.version));
	retry.data_size = sizeof(retry);
	strcpy(retry.name, retry_name);
	TEST_RES(ioctl(fd, DM_DEV_CREATE, &retry),
		 _ret == 0 && retry.dev != 0 && retry.dev != io.dev);

	struct dm_ioctl invalid = {};
	memcpy(invalid.version, kernel_version, sizeof(invalid.version));
	invalid.data_size = sizeof(invalid);
	strcpy(invalid.name, "invalid/name");
	TEST(ioctl(fd, DM_DEV_CREATE, &invalid), EINVAL, _ret == -1);

	TEST_SUCC(close(fd));
}
END_TEST()

/* No REMOVE yet; use the official fresh, unformatted /dev/vdd disk. */
FN_TEST(device_mapper_first_resume_linear_io)
{
	char name[DM_NAME_LEN];
	char primary[64];
	union dm_table_buffer table;
	struct dm_ioctl create = {}, resume = {}, repeat_resume = {};
	unsigned char written[512], observed[512], zero[512] = {};
	int control = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));

	snprintf(name, sizeof(name), "dm-resume-%ld", (long)getpid());
	memcpy(create.version, kernel_version, sizeof(create.version));
	create.data_size = sizeof(create);
	strcpy(create.name, name);
	TEST_SUCC(ioctl(control, DM_DEV_CREATE, &create));
	snprintf(primary, sizeof(primary), "/dev/dm-%u", minor(create.dev));

	memcpy(resume.version, kernel_version, sizeof(resume.version));
	resume.data_size = sizeof(resume);
	strcpy(resume.name, name);
	TEST(ioctl(control, DM_DEV_SUSPEND, &resume), EINVAL, _ret == -1);
	init_linear_table(table.bytes, sizeof(table.bytes), name, 8);
	TEST_SUCC(ioctl(control, DM_TABLE_LOAD, table.bytes));
	TEST_RES(table.align.flags & DM_INACTIVE_PRESENT_FLAG, _ret != 0);
	resume.flags = DM_SUSPEND_FLAG;
	TEST(ioctl(control, DM_DEV_SUSPEND, &resume), EINVAL, _ret == -1);
	resume.flags = 0;
	TEST_SUCC(ioctl(control, DM_DEV_SUSPEND, &resume));
	TEST_RES(resume.flags & DM_ACTIVE_PRESENT_FLAG, _ret != 0);
	TEST_RES(resume.flags & DM_INACTIVE_PRESENT_FLAG, _ret == 0);
	TEST_RES(resume.target_count, _ret == 1);
	/* Use a fresh request: ioctl replies contain state and a device number. */
	memcpy(repeat_resume.version, kernel_version,
	       sizeof(repeat_resume.version));
	repeat_resume.data_size = sizeof(repeat_resume);
	strcpy(repeat_resume.name, name);
	TEST_SUCC(ioctl(control, DM_DEV_SUSPEND, &repeat_resume));

	int mapped = CHECK(open(primary, O_RDWR | O_CLOEXEC));
	int backing = CHECK(open("/dev/vdd", O_RDONLY | O_CLOEXEC));
	/* The official regression entry recreates the image before QEMU starts. */
	TEST_RES(pread(backing, observed, sizeof(observed), 512),
		 _ret == sizeof(observed));
	TEST_RES(memcmp(zero, observed, sizeof(zero)), _ret == 0);
	memset(written, 0xa7, sizeof(written));
	TEST_RES(pwrite(mapped, written, sizeof(written), 512),
		 _ret == sizeof(written));
	TEST_RES(pread(backing, observed, sizeof(observed), 512),
		 _ret == sizeof(observed));
	TEST_RES(memcmp(written, observed, sizeof(written)), _ret == 0);
	memset(observed, 0, sizeof(observed));
	TEST_RES(pread(mapped, observed, sizeof(observed), 512),
		 _ret == sizeof(observed));
	TEST_RES(memcmp(written, observed, sizeof(written)), _ret == 0);
	TEST_SUCC(close(backing));
	TEST_SUCC(close(mapped));
	TEST_SUCC(close(control));
}
END_TEST()

/* No REMOVE command yet: the mapper and backing lease remain until reboot.
 * Run with the official fresh /dev/vdd regression disk present.
 */
/* A sector-aligned request becomes one BIO in the block-device VmIo path.
 * Crossing sector 4 therefore exercises DM splitting, not two user requests.
 */
static void check_multi_target_linear_io(const char *second_path,
					 unsigned int first_sector,
					 unsigned int second_sector,
					 const char *name_prefix, int *passes,
					 int *failures)
{
	int __tests_passed = 0, __tests_failed = 0;
	char name[DM_NAME_LEN], primary[64];
	union dm_table_buffer table;
	struct dm_ioctl create = {}, resume = {};
	unsigned char written[4096], observed[4096], zero[512] = {};
	const off_t offsets[] = { first_sector * 512, second_sector * 512 };
	const off_t guards[] = { offsets[0] - 512, offsets[0] + 2048,
				 offsets[1] - 512, offsets[1] + 2048 };
	int control = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));
	int backing[] = { CHECK(open("/dev/vdd", O_RDWR | O_CLOEXEC)),
			  CHECK(open(second_path, O_RDWR | O_CLOEXEC)) };
	struct stat first_stat, second_stat;
	TEST_SUCC(fstat(backing[0], &first_stat));
	TEST_SUCC(fstat(backing[1], &second_stat));
	if (strcmp(second_path, "/dev/vdd") != 0)
		TEST_RES(first_stat.st_rdev != second_stat.st_rdev, _ret != 0);

	snprintf(name, sizeof(name), "%s-%ld", name_prefix, (long)getpid());
	memcpy(create.version, kernel_version, sizeof(create.version));
	create.data_size = sizeof(create);
	strcpy(create.name, name);
	TEST_SUCC(ioctl(control, DM_DEV_CREATE, &create));
	snprintf(primary, sizeof(primary), "/dev/dm-%u", minor(create.dev));
	init_linear_table(table.bytes, sizeof(table.bytes), name, 4);
	struct dm_target_spec *first =
		(struct dm_target_spec *)(table.bytes + table.align.data_start);
	snprintf((char *)(first + 1), 64, "/dev/vdd %u", first_sector);
	first->next =
		align_to_u64(sizeof(*first) + strlen((char *)(first + 1)) + 1);
	struct dm_target_spec *second =
		(struct dm_target_spec *)((unsigned char *)first + first->next);
	second->sector_start = 4;
	second->length = 4;
	strcpy(second->target_type, "linear");
	snprintf((char *)(second + 1), 64, "%s %u", second_path, second_sector);
	table.align.target_count = 2;
	table.align.data_size = table.align.data_start + first->next +
				align_to_u64(sizeof(*second) +
					     strlen((char *)(second + 1)) + 1);
	TEST_SUCC(ioctl(control, DM_TABLE_LOAD, table.bytes));
	memcpy(resume.version, kernel_version, sizeof(resume.version));
	resume.data_size = sizeof(resume);
	strcpy(resume.name, name);
	TEST_SUCC(ioctl(control, DM_DEV_SUSPEND, &resume));
	TEST_RES(resume.target_count, _ret == 2);
	int mapped = CHECK(open(primary, O_RDWR | O_CLOEXEC));

	/* Check fresh backing ranges, then write distinct data to every sector. */
	for (size_t i = 0; i < 2; ++i) {
		TEST_RES(pread(backing[i], observed, 2048, offsets[i]),
			 _ret == 2048);
		for (size_t j = 0; j < 2048; j += 512)
			TEST_RES(memcmp(observed + j, zero, 512), _ret == 0);
	}
	for (size_t i = 0; i < sizeof(written); ++i)
		written[i] = (unsigned char)(0x31 + i / 512);
	TEST_RES(pwrite(mapped, written, sizeof(written), 0),
		 _ret == sizeof(written));
	for (size_t i = 0; i < 2; ++i) {
		TEST_RES(pread(backing[i], observed, 2048, offsets[i]),
			 _ret == 2048);
		TEST_RES(memcmp(observed, written + i * 2048, 2048), _ret == 0);
	}
	TEST_RES(pread(mapped, observed, sizeof(observed), 0),
		 _ret == sizeof(observed));
	TEST_RES(memcmp(observed, written, sizeof(written)), _ret == 0);

	/* Seed the backing directly to independently verify cross-target reads. */
	for (size_t i = 0; i < sizeof(written); ++i)
		written[i] = (unsigned char)(0xb1 + i / 512);
	for (size_t i = 0; i < 2; ++i)
		TEST_RES(pwrite(backing[i], written + i * 2048, 2048,
				offsets[i]),
			 _ret == 2048);
	TEST_RES(pread(mapped, observed, sizeof(observed), 0),
		 _ret == sizeof(observed));
	TEST_RES(memcmp(observed, written, sizeof(written)), _ret == 0);
	/* A partial request crosses the boundary without touching outer sectors. */
	memset(observed, 0x6d, sizeof(observed));
	TEST_RES(pread(mapped, observed, 1024, 3 * 512), _ret == 1024);
	TEST_RES(memcmp(observed, written + 3 * 512, 1024), _ret == 0);
	TEST_RES(observed[1024], _ret == 0x6d);
	unsigned char partial[1024];
	memset(partial, 0x79, 512);
	memset(partial + 512, 0xc3, 512);
	TEST_RES(pwrite(mapped, partial, sizeof(partial), 3 * 512),
		 _ret == sizeof(partial));
	memcpy(written + 3 * 512, partial, sizeof(partial));
	for (size_t i = 0; i < 2; ++i) {
		TEST_RES(pread(backing[i], observed, 2048, offsets[i]),
			 _ret == 2048);
		TEST_RES(memcmp(observed, written + i * 2048, 2048), _ret == 0);
	}
	TEST_RES(pread(mapped, observed, sizeof(observed), 0),
		 _ret == sizeof(observed));
	TEST_RES(memcmp(observed, written, sizeof(written)), _ret == 0);
	for (size_t i = 0; i < sizeof(guards) / sizeof(guards[0]); ++i) {
		TEST_RES(pread(backing[i / 2], observed, 512, guards[i]),
			 _ret == 512);
		TEST_RES(memcmp(observed, zero, 512), _ret == 0);
	}
	TEST_SUCC(close(mapped));
	TEST_SUCC(close(backing[0]));
	TEST_SUCC(close(backing[1]));
	TEST_SUCC(close(control));
	*passes += __tests_passed;
	*failures += __tests_failed;
}

FN_TEST(device_mapper_multi_target_linear_io)
{
	check_multi_target_linear_io("/dev/vdd", 32, 64, "dm-multi",
				     &__tests_passed, &__tests_failed);
}
END_TEST()

/* Requires a second fresh raw disk from raw_disk=2. */
FN_TEST(device_mapper_multi_disk_linear_io)
{
	check_multi_target_linear_io("/dev/vde", 96, 128, "dm-two-disk",
				     &__tests_passed, &__tests_failed);
}
END_TEST()

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

/* These checks need an active mapper and belong to the RESUME milestone. */
FN_TEST(device_mapper_active_table_failure_and_bounds)
{
	char name[DM_NAME_LEN], primary[64], backing_id[32];
	unsigned char expected[4096], observed[4096], replacement[1024];
	union dm_table_buffer table;
	struct dm_ioctl create = {}, resume = {};
	struct stat st;
	int control = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));
	int backing = CHECK(open("/dev/vdd", O_RDWR | O_CLOEXEC));
	CHECK(fstat(backing, &st));
	snprintf(backing_id, sizeof(backing_id), "%u:%u", major(st.st_rdev),
		 minor(st.st_rdev));
	snprintf(name, sizeof(name), "dm-active-check-%ld", (long)getpid());
	memcpy(create.version, kernel_version, sizeof(create.version));
	create.data_size = sizeof(create);
	strcpy(create.name, name);
	CHECK(ioctl(control, DM_DEV_CREATE, &create));
	snprintf(primary, sizeof(primary), "/dev/dm-%u", minor(create.dev));
	memset(expected, 0x53, sizeof(expected));
	CHECK_WITH(pwrite(backing, expected, sizeof(expected), 192 * 512),
		   _ret == sizeof(expected));
	init_linear_table_params(table.bytes, sizeof(table.bytes), name, 8,
				 backing_id, 192);
	CHECK(ioctl(control, DM_TABLE_LOAD, table.bytes));
	/* An invalid candidate must preserve the original inactive table. */
	init_linear_table_params(table.bytes, sizeof(table.bytes), name, 8,
				 backing_id, 224);
	table.align.target_count = 0;
	TEST(ioctl(control, DM_TABLE_LOAD, table.bytes), EINVAL, _ret == -1);
	memcpy(resume.version, kernel_version, sizeof(resume.version));
	resume.data_size = sizeof(resume);
	strcpy(resume.name, name);
	CHECK(ioctl(control, DM_DEV_SUSPEND, &resume));
	int mapped = CHECK(open(primary, O_RDWR | O_CLOEXEC));
	TEST_RES(pread(mapped, observed, sizeof(observed), 0),
		 _ret == sizeof(observed));
	TEST_RES(memcmp(observed, expected, sizeof(expected)), _ret == 0);

	init_linear_table_params(table.bytes, sizeof(table.bytes), name, 8,
				 backing_id, 224);
	table.align.target_count = 0;
	TEST(ioctl(control, DM_TABLE_LOAD, table.bytes), EINVAL, _ret == -1);
	TEST_RES(pread(mapped, observed, sizeof(observed), 0),
		 _ret == sizeof(observed));
	TEST_RES(memcmp(observed, expected, sizeof(expected)), _ret == 0);
	init_linear_table_params(table.bytes, sizeof(table.bytes), name, 8,
				 backing_id, 224);
	CHECK(ioctl(control, DM_TABLE_LOAD, table.bytes));
	memset(&resume, 0, sizeof(resume));
	memcpy(resume.version, kernel_version, sizeof(resume.version));
	resume.data_size = sizeof(resume);
	strcpy(resume.name, name);
	TEST(ioctl(control, DM_DEV_SUSPEND, &resume), EINVAL, _ret == -1);
	TEST_RES(pread(mapped, observed, sizeof(observed), 0),
		 _ret == sizeof(observed));
	TEST_RES(memcmp(observed, expected, sizeof(expected)), _ret == 0);
	memset(replacement, 0x97, sizeof(replacement));
	TEST_RES(pwrite(mapped, replacement, 512, 0), _ret == 512);
	memcpy(expected, replacement, 512);
	TEST_RES(pread(backing, observed, sizeof(observed), 192 * 512),
		 _ret == sizeof(observed));
	TEST_RES(memcmp(observed, expected, sizeof(expected)), _ret == 0);
	unsigned char zero[4096] = {};
	TEST_RES(pread(backing, observed, sizeof(observed), 224 * 512),
		 _ret == sizeof(observed));
	TEST_RES(memcmp(observed, zero, sizeof(zero)), _ret == 0);

	memset(observed, 0x6d, sizeof(observed));
	TEST_RES(pread(mapped, observed, 1024, 7 * 512), _ret == 512);
	TEST_RES(memcmp(observed, expected + 7 * 512, 512), _ret == 0);
	TEST_RES(observed[512], _ret == 0x6d);
	TEST_RES(pwrite(mapped, replacement, 1024, 7 * 512), _ret == 512);
	memcpy(expected + 7 * 512, replacement, 512);
	TEST_RES(pread(mapped, observed, 512, 8 * 512), _ret == 0);
	TEST(pwrite(mapped, replacement, 512, 8 * 512), ENOSPC, _ret == -1);
	TEST_RES(pread(mapped, observed, 512, 9 * 512), _ret == 0);
	TEST(pwrite(mapped, replacement, 512, 9 * 512), ENOSPC, _ret == -1);
	TEST_RES(pread(backing, observed, sizeof(observed), 192 * 512),
		 _ret == sizeof(observed));
	TEST_RES(memcmp(observed, expected, sizeof(expected)), _ret == 0);
	TEST_RES(pread(backing, observed, 512, 200 * 512), _ret == 512);
	TEST_RES(memcmp(observed, zero, 512), _ret == 0);
	TEST_SUCC(close(mapped));
	TEST_SUCC(close(backing));
	TEST_SUCC(close(control));
}
END_TEST()

FN_TEST(device_mapper_readonly_linear_io)
{
	char name[DM_NAME_LEN], primary[64];
	unsigned char expected[512], observed[512], attempted[512];
	union dm_table_buffer table;
	struct dm_ioctl create = {}, resume = {};
	int control = CHECK(open(DM_CONTROL_PATH, O_RDWR | O_CLOEXEC));
	int backing = CHECK(open("/dev/vdd", O_RDWR | O_CLOEXEC));
	memset(expected, 0x42, sizeof(expected));
	memset(attempted, 0xe1, sizeof(attempted));
	CHECK_WITH(pwrite(backing, expected, sizeof(expected), 256 * 512),
		   _ret == sizeof(expected));
	snprintf(name, sizeof(name), "dm-readonly-%ld", (long)getpid());
	memcpy(create.version, kernel_version, sizeof(create.version));
	create.data_size = sizeof(create);
	strcpy(create.name, name);
	CHECK(ioctl(control, DM_DEV_CREATE, &create));
	snprintf(primary, sizeof(primary), "/dev/dm-%u", minor(create.dev));
	init_linear_table_params(table.bytes, sizeof(table.bytes), name, 8,
				 "/dev/vdd", 256);
	table.align.flags = DM_READONLY_FLAG;
	CHECK(ioctl(control, DM_TABLE_LOAD, table.bytes));
	memcpy(resume.version, kernel_version, sizeof(resume.version));
	resume.data_size = sizeof(resume);
	strcpy(resume.name, name);
	TEST_RES(ioctl(control, DM_DEV_SUSPEND, &resume), _ret == 0);
	TEST_RES(resume.flags & DM_READONLY_FLAG, _ret != 0);
	int mapped = CHECK(open(primary, O_RDWR | O_CLOEXEC));
	TEST_RES(pread(mapped, observed, sizeof(observed), 0),
		 _ret == sizeof(observed));
	TEST_RES(memcmp(observed, expected, sizeof(expected)), _ret == 0);
	TEST(pwrite(mapped, attempted, sizeof(attempted), 0), EBUSY,
	     _ret == -1);
	TEST_RES(pread(backing, observed, sizeof(observed), 256 * 512),
		 _ret == sizeof(observed));
	TEST_RES(memcmp(observed, expected, sizeof(expected)), _ret == 0);
	TEST_RES(pread(mapped, observed, sizeof(observed), 0),
		 _ret == sizeof(observed));
	TEST_RES(memcmp(observed, expected, sizeof(expected)), _ret == 0);
	TEST_SUCC(close(mapped));
	TEST_SUCC(close(backing));
	TEST_SUCC(close(control));
}
END_TEST()
