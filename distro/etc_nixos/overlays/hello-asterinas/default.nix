final: prev: {
  hello-asterinas = prev.stdenv.mkDerivation {
    name = "hello-asterinas";
    version = "0.1.0";
    buildCommand = ''
      mkdir -p $out/bin
      cat > $out/bin/hello-asterinas << 'EOF'
      #!/bin/sh
      echo "Hello Asterinas!"
      EOF
      chmod +x $out/bin/hello-asterinas
    '';
  };

  aster-dm-disk-locator = prev.stdenv.mkDerivation {
    name = "aster-dm-disk-locator";
    version = "0.1.0";
    dontUnpack = true;
    buildPhase = ''
      cat > aster-dm-disk-locator.c << 'EOF'
      // SPDX-License-Identifier: MPL-2.0

      #define _GNU_SOURCE

      #include <errno.h>
      #include <fcntl.h>
      #include <linux/ioctl.h>
      #include <stdio.h>
      #include <string.h>
      #include <sys/ioctl.h>
      #include <unistd.h>

      #define ASTER_VIRTIO_BLK_ID_BYTES 20
      #define ASTER_VIRTIO_BLK_GET_ID \
        _IOR('A', 0x01, unsigned char[ASTER_VIRTIO_BLK_ID_BYTES])
      #define EXPECTED_ID "vdmtest"
      #define MAX_VIRTIO_DISKS 702

      static void format_device_name(unsigned int index, char *path,
                                     size_t capacity)
      {
        char suffix[8];
        size_t suffix_len = 0;

        do {
          suffix[suffix_len++] = 'a' + index % 26;
          index /= 26;
          if (index == 0) {
            break;
          }
          index--;
        } while (suffix_len < sizeof(suffix));

        size_t written = (size_t)snprintf(path, capacity, "/dev/vd");
        while (suffix_len > 0 && written + 1 < capacity) {
          path[written++] = suffix[--suffix_len];
        }
        path[written] = '\0';
      }

      int main(void)
      {
        char matched_path[32] = { 0 };
        unsigned int matches = 0;

        for (unsigned int index = 0; index < MAX_VIRTIO_DISKS; index++) {
          unsigned char id[ASTER_VIRTIO_BLK_ID_BYTES];
          char path[32];
          int fd;

          format_device_name(index, path, sizeof(path));
          fd = open(path, O_RDONLY | O_CLOEXEC);
          if (fd < 0) {
            if (errno == ENOENT || errno == ENXIO || errno == ENODEV) {
              continue;
            }
            fprintf(stderr, "无法只读打开 %s: %s\n", path, strerror(errno));
            return 1;
          }

          if (ioctl(fd, ASTER_VIRTIO_BLK_GET_ID, id) == 0) {
            size_t id_len = 0;
            while (id_len < sizeof(id) && id[id_len] != '\0') {
              id_len++;
            }
            if (id_len == sizeof(EXPECTED_ID) - 1 &&
                memcmp(id, EXPECTED_ID, id_len) == 0) {
              matches++;
              snprintf(matched_path, sizeof(matched_path), "%s", path);
            }
          } else if (errno != ENOTTY && errno != ENODATA) {
            fprintf(stderr, "读取 %s 的 VirtIO Host ID 失败: %s\n", path,
                    strerror(errno));
            close(fd);
            return 1;
          }

          if (close(fd) < 0) {
            fprintf(stderr, "关闭 %s 失败: %s\n", path, strerror(errno));
            return 1;
          }
        }

        if (matches != 1) {
          fprintf(stderr, "必须且只能找到一个 ID 为 %s 的 VirtIO 整盘，实际找到 %u 个\n",
                  EXPECTED_ID, matches);
          return 1;
        }

        puts(matched_path);
        return 0;
      }
      EOF

      $CC -std=c11 -Wall -Wextra -Werror -O2 \
        aster-dm-disk-locator.c -o aster-dm-disk-locator
    '';
    installPhase = ''
      mkdir -p $out/bin
      install -m 755 aster-dm-disk-locator $out/bin/aster-dm-disk-locator
    '';
  };
}
