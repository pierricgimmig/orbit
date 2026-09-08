// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license in LICENSE.
// Darwin bootstrap: let dyld register the Rust agent's TLS and load dependencies.
#include <dlfcn.h>
#include <limits.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

static void report(const char *path, const char *error) {
  int fd = socket(AF_UNIX, SOCK_STREAM, 0);
  if (fd < 0) return;
  int enabled = 1;
  setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &enabled, sizeof(enabled));
  struct sockaddr_un addr = {0};
  addr.sun_family = AF_UNIX;
  if (strlen(path) >= sizeof(addr.sun_path)) { close(fd); return; }
  strcpy(addr.sun_path, path);
  if (connect(fd, (struct sockaddr *)&addr, sizeof(addr)) == 0) {
    char message[2048]; size_t n = 0;
    const char *prefix = "{\"error\":\"native agent loader: ";
    memcpy(message, prefix, strlen(prefix)); n = strlen(prefix);
    for (const unsigned char *p = (const void *)error; *p && n < sizeof(message) - 8; p++) {
      if (*p == '"' || *p == '\\') message[n++] = '\\';
      message[n++] = *p < 32 ? ' ' : *p;
    }
    memcpy(message + n, "\"}\n", 3); n += 3;
    (void)write(fd, message, n);
  }
  close(fd);
}
__attribute__((visibility("default")))
void orbit_frida_load(const char *data, int *unload_policy, void *state) {
  *unload_policy = 0; // Only this bootstrap can unload; the dlopen handle stays.
  char *end;
  unsigned long length = strtoul(data, &end, 10);
  if (end == data || *end != ':' || length == 0 || length >= PATH_MAX ||
      strlen(end + 1) <= length) return;
  const char *socket_path = end + 1 + length;
  char path[PATH_MAX];
  memcpy(path, end + 1, length); path[length] = 0;
  void *module = dlopen(path, RTLD_NOW | RTLD_LOCAL);
  if (module == NULL) { report(socket_path, dlerror()); return; }
  void (*entry)(const char *, int *, void *) = dlsym(module, "orbit_frida_main");
  if (entry == NULL) { report(socket_path, dlerror()); dlclose(module); return; }
  int agent_policy = 1;
  entry(socket_path, &agent_policy, state);
  // No dlclose: outstanding Gum listeners and the bundled manual SDK are live.
}
