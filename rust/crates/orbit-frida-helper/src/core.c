// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "frida-core.h"
#include <stdio.h>

/* Only the native injector is used: no Frida session, script, or GumJS agent. */
void * orbit_core_inject(unsigned pid, const char * path, const char * data,
    char * message, size_t capacity) {
  GError * error = NULL;
  frida_init();
  FridaInjector * injector = frida_injector_new();
  frida_injector_inject_library_file_sync(injector, pid, path,
      "orbit_frida_main", data, NULL, &error);
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
