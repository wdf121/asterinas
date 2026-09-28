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

  aster-test-disk-locator = prev.stdenv.mkDerivation {
    name = "aster-test-disk-locator";
    version = "0.1.0";
    dontUnpack = true;
    buildPhase = ''
      cat > aster-test-disk-locator.c << 'EOF'
      // SPDX-License-Identifier: MPL-2.0

      #include <errno.h>
      #include <limits.h>
      #include <stdio.h>
      #include <stdlib.h>
      #include <string.h>
      #include <sys/stat.h>

      #define CMDLINE_PATH "/proc/cmdline"
      #define FIRST_DISK_PARAM "aster.test_disk_first="
      #define DEVICE_PREFIX "/dev/vd"
      #define PATH_CAPACITY 32

      static int read_first_disk(char *path, size_t capacity)
      {
        FILE *cmdline = fopen(CMDLINE_PATH, "r");
        char token[256];
        const size_t param_len = sizeof(FIRST_DISK_PARAM) - 1;

        if (cmdline == NULL) {
          fprintf(stderr, "无法读取 %s: %s\n", CMDLINE_PATH, strerror(errno));
          return -1;
        }

        while (fscanf(cmdline, "%255s", token) == 1) {
          if (strncmp(token, FIRST_DISK_PARAM, param_len) != 0) {
            continue;
          }

          const char *value = token + param_len;
          size_t value_len = strlen(value);
          if (value_len == 0 || value_len >= capacity) {
            fprintf(stderr, "%s 的测试盘路径无效\n", FIRST_DISK_PARAM);
            fclose(cmdline);
            return -1;
          }

          memcpy(path, value, value_len + 1);
          fclose(cmdline);
          return 0;
        }

        fprintf(stderr, "命令行缺少 %s 参数\n", FIRST_DISK_PARAM);
        fclose(cmdline);
        return -1;
      }

      static int parse_device_index(const char *path, unsigned int *index)
      {
        const char *suffix;
        unsigned long value = 0;

        if (strncmp(path, DEVICE_PREFIX, sizeof(DEVICE_PREFIX) - 1) != 0) {
          return -1;
        }
        suffix = path + sizeof(DEVICE_PREFIX) - 1;
        if (*suffix == '\0') {
          return -1;
        }

        while (*suffix != '\0') {
          unsigned long digit;
          if (*suffix < 'a' || *suffix > 'z') {
            return -1;
          }
          digit = (unsigned long)(*suffix - 'a') + 1;
          if (value > (UINT_MAX - digit) / 26) {
            return -1;
          }
          value = value * 26 + digit;
          suffix++;
        }

        *index = (unsigned int)(value - 1);
        return 0;
      }

      static int format_device_name(unsigned int index, char *path, size_t capacity)
      {
        char suffix[8];
        size_t suffix_len = 0;
        size_t written;

        do {
          if (suffix_len == sizeof(suffix)) {
            return -1;
          }
          suffix[suffix_len++] = (char)('a' + index % 26);
          index /= 26;
          if (index == 0) {
            break;
          }
          index--;
        } while (1);

        written = (size_t)snprintf(path, capacity, DEVICE_PREFIX);
        if (written >= capacity || written + suffix_len >= capacity) {
          return -1;
        }
        while (suffix_len > 0) {
          path[written++] = suffix[--suffix_len];
        }
        path[written] = '\0';
        return 0;
      }

      static int parse_ordinal(int argc, char *argv[], unsigned int *ordinal)
      {
        char *end;
        unsigned long value;

        if (argc == 1) {
          *ordinal = 1;
          return 0;
        }
        if (argc != 2) {
          fprintf(stderr, "用法: %s [测试盘序号]\n", argv[0]);
          return -1;
        }

        errno = 0;
        value = strtoul(argv[1], &end, 10);
        if (errno != 0 || *argv[1] == '\0' || *end != '\0' || value == 0 ||
            value > UINT_MAX) {
          fprintf(stderr, "测试盘序号必须是正整数\n");
          return -1;
        }

        *ordinal = (unsigned int)value;
        return 0;
      }

      int main(int argc, char *argv[])
      {
        char first_disk[PATH_CAPACITY];
        char disk[PATH_CAPACITY];
        unsigned int first_index;
        unsigned int ordinal;
        struct stat statbuf;

        if (parse_ordinal(argc, argv, &ordinal) != 0 ||
            read_first_disk(first_disk, sizeof(first_disk)) != 0) {
          return 1;
        }
        if (parse_device_index(first_disk, &first_index) != 0 ||
            ordinal - 1 > UINT_MAX - first_index ||
            format_device_name(first_index + ordinal - 1, disk, sizeof(disk)) != 0) {
          fprintf(stderr, "无法从 %s=%s 推导测试盘路径\n", FIRST_DISK_PARAM,
                  first_disk);
          return 1;
        }
        if (stat(disk, &statbuf) != 0) {
          fprintf(stderr, "测试盘 %s 不存在: %s\n", disk, strerror(errno));
          return 1;
        }
        if (!S_ISBLK(statbuf.st_mode)) {
          fprintf(stderr, "测试盘 %s 不是块设备\n", disk);
          return 1;
        }

        puts(disk);
        return 0;
      }
      EOF

      $CC -std=c11 -Wall -Wextra -Werror -O2 \
        aster-test-disk-locator.c -o aster-test-disk-locator
    '';
    installPhase = ''
      mkdir -p $out/bin
      install -m 755 aster-test-disk-locator $out/bin/aster-test-disk-locator
    '';
  };
}
