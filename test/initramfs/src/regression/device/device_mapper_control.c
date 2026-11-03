// SPDX-License-Identifier: MPL-2.0

/* VERSION-only Device Mapper control ABI regression tests. */

#include <fcntl.h>
#include <linux/dm-ioctl.h>
#include <stdint.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <unistd.h>

#include "../common/test.h"

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
