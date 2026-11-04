// SPDX-License-Identifier: MPL-2.0

/* LOAD request builder shared by the DM ABI regressions.
 * The official initramfs regression entry supplies an unformatted /dev/vdd.
 */
#ifndef DEVICE_MAPPER_COMMON_H
#define DEVICE_MAPPER_COMMON_H

#include <linux/dm-ioctl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define DM_TABLE_BUFFER_SIZE 512

union dm_table_buffer {
	struct dm_ioctl align;
	unsigned char bytes[DM_TABLE_BUFFER_SIZE];
};

static inline size_t align_to_u64(size_t value)
{
	return (value + sizeof(uint64_t) - 1) & ~(sizeof(uint64_t) - 1);
}

static inline void init_linear_table_params(unsigned char *buffer,
					    size_t buffer_size,
					    const char *name, uint64_t length,
					    const char *backing,
					    unsigned int offset)
{
	struct dm_ioctl *io = (struct dm_ioctl *)buffer;
	struct dm_target_spec *target;
	char *params;
	size_t record_size;

	memset(buffer, 0, buffer_size);
	io->version[0] = 4;
	io->version[1] = 48;
	io->data_size = sizeof(*io);
	io->data_start = sizeof(*io);
	snprintf(io->name, sizeof(io->name), "%s", name);
	target = (struct dm_target_spec *)(buffer + io->data_start);
	target->sector_start = 0;
	target->length = length;
	target->next = 0;
	snprintf(target->target_type, sizeof(target->target_type), "linear");
	params = (char *)(target + 1);
	snprintf(params, buffer_size - io->data_start - sizeof(*target),
		 "%s %u", backing, offset);
	record_size = sizeof(*target) + strlen(params) + 1;
	io->target_count = 1;
	io->data_size = io->data_start + align_to_u64(record_size);
}

static inline void init_linear_table(unsigned char *buffer, size_t buffer_size,
				     const char *name, uint64_t length)
{
	init_linear_table_params(buffer, buffer_size, name, length, "/dev/vdd",
				 0);
}

#endif /* DEVICE_MAPPER_COMMON_H */
