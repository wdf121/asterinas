// SPDX-License-Identifier: MPL-2.0

#define _GNU_SOURCE

#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "../../common/test.h"

#define READ_CHUNK_SIZE 7

static ssize_t read_all_in_chunks(int fd, char *buf, size_t capacity)
{
	size_t bytes_read = 0;

	while (bytes_read < capacity) {
		size_t remaining = capacity - bytes_read;
		size_t read_size = remaining < READ_CHUNK_SIZE ?
					   remaining :
					   READ_CHUNK_SIZE;
		ssize_t bytes = read(fd, buf + bytes_read, read_size);

		if (bytes <= 0) {
			return bytes < 0 ? bytes : (ssize_t)bytes_read;
		}
		bytes_read += bytes;
	}

	return bytes_read;
}

static size_t count_substrings(const char *text, const char *needle)
{
	size_t count = 0;
	const char *cursor = text;

	while ((cursor = strstr(cursor, needle)) != NULL) {
		count++;
		cursor += strlen(needle);
	}

	return count;
}

FN_TEST(devices_reports_device_mapper_major)
{
	char buf[4096] = { 0 };
	int fd = TEST_SUCC(open("/proc/devices", O_RDONLY));

	// 使用短读覆盖 procfs 的 offset 续读行为。
	TEST_RES(read_all_in_chunks(fd, buf, sizeof(buf) - 1), _ret > 0);

	const char *character = strstr(buf, "Character devices:\n");
	const char *block = strstr(buf, "Block devices:\n");
	const char *virtblk = strstr(buf, " virtblk\n");
	const char *mapper = strstr(buf, " device-mapper\n");
	unsigned int virtblk_major = 0;
	unsigned int mapper_major = 0;

	TEST_RES(character != NULL && block != NULL && character < block,
		 _ret == 1);
	TEST_RES(count_substrings(buf, " virtblk\n"), _ret == 1);
	TEST_RES(virtblk != NULL, _ret == 1);
	if (virtblk != NULL) {
		const char *line = virtblk;

		while (line > buf && line[-1] != '\n') {
			line--;
		}
		TEST_RES(sscanf(line, "%u virtblk", &virtblk_major), _ret == 1);
	}
	TEST_RES(virtblk_major, _ret > 0 && _ret <= 4095);

	TEST_RES(count_substrings(buf, " device-mapper\n"), _ret == 1);
	TEST_RES(mapper != NULL, _ret == 1);
	if (mapper != NULL) {
		const char *line = mapper;

		while (line > buf && line[-1] != '\n') {
			line--;
		}
		TEST_RES(sscanf(line, "%u device-mapper", &mapper_major), _ret == 1);
	}
	TEST_RES(mapper_major, _ret > 0 && _ret <= 4095);

	TEST_SUCC(close(fd));
}
END_TEST()
