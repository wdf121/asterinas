// SPDX-License-Identifier: MPL-2.0

/* Shared request builders and raw ABI readers for the DM regression tests.
 * Builders require buffers large enough for the header and target payload;
 * they only prepare requests and do not create or own kernel resources.
 */
#ifndef DEVICE_MAPPER_COMMON_H
#define DEVICE_MAPPER_COMMON_H

#include <linux/dm-ioctl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/sysmacros.h>
#include <sys/types.h>

#define DM_CONTROL_PATH "/dev/mapper/control"
#define DM_IOCTL_ENVELOPE_SIZE 312
#define DM_TABLE_BUFFER_SIZE 512
#define DM_IOCTL_FIXED_SIZE 305

union dm_table_buffer {
	struct dm_ioctl align;
	unsigned char bytes[DM_TABLE_BUFFER_SIZE];
};

static inline void init_ioctl(struct dm_ioctl *io)
{
	memset(io, 0, sizeof(*io));
	io->version[0] = DM_VERSION_MAJOR;
	io->version[1] = DM_VERSION_MINOR;
	io->version[2] = DM_VERSION_PATCHLEVEL;
	io->data_size = sizeof(*io);
	io->data_start = sizeof(*io);
}

static inline void init_named_ioctl(struct dm_ioctl *io, const char *name)
{
	init_ioctl(io);
	snprintf(io->name, sizeof(io->name), "%s", name);
}

static inline size_t align_to_u64(size_t value)
{
	return (value + sizeof(uint64_t) - 1) & ~(sizeof(uint64_t) - 1);
}

/* Raw readers avoid alignment assumptions for guard-page ioctl arguments. */
static inline uint32_t raw_u32(const unsigned char *bytes)
{
	uint32_t value;
	memcpy(&value, bytes, sizeof(value));
	return value;
}

static inline uint64_t raw_u64(const unsigned char *bytes)
{
	uint64_t value;
	memcpy(&value, bytes, sizeof(value));
	return value;
}

static inline void init_linear_table(unsigned char *buffer, size_t buffer_size,
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

static inline void init_zero_table(unsigned char *buffer, size_t buffer_size,
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

static inline void mapper_paths(uint64_t dev, const char *name, char *primary,
				size_t primary_size, char *alias,
				size_t alias_size)
{
	snprintf(primary, primary_size, "/dev/dm-%u", minor((dev_t)dev));
	snprintf(alias, alias_size, "/dev/mapper/%s", name);
}

#endif /* DEVICE_MAPPER_COMMON_H */
