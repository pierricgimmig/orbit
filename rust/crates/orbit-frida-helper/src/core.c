// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "frida-core.h"
#include <stdio.h>

/* Only the native injector is used: no Frida session, script, or GumJS agent. */
void * orbit_core_inject(unsigned pid, const char * path, const char * data,
    char * message, size_t capacity, const void * loader, size_t loader_size) {
  GError * error = NULL;
  frida_init();
  FridaInjector * injector = frida_injector_new();
#ifdef __APPLE__
  // Core's custom mapper does not register Rust's Mach-O thread-local storage
  // with dyld. Inject a libSystem-only bootstrap which dlopens the real agent.
  GBytes * blob = g_bytes_new_static(loader, loader_size);
  gchar * config = g_strdup_printf("%zu:%s%s", strlen(path), path, data);
  frida_injector_inject_library_blob_sync(injector, pid, blob,
      "orbit_frida_load", config, NULL, &error);
  g_free(config);
  g_bytes_unref(blob);
#else
  (void)loader; (void)loader_size;
  frida_injector_inject_library_file_sync(injector, pid, path,
      "orbit_frida_main", data, NULL, &error);
#endif
  if (error != NULL) {
    snprintf(message, capacity, "%s", error->message);
    g_error_free(error);
    frida_injector_close_sync(injector, NULL, NULL);
    g_object_unref(injector);
    return NULL;
  }
  return injector;
}
void orbit_core_close(void * injector) {
  frida_injector_close_sync(injector, NULL, NULL);
  g_object_unref(injector);
  /* The helper exits next; no global deinit while Core's worker is running. */
}
