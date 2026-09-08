// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Thin ABI adapter; Gum owns code generation, relocation and listener lifetime.
#include "frida-gum.h"
#include <pthread.h>
#include <stdint.h>
#include <string.h>
#ifdef __APPLE__
#include <mach-o/loader.h>
#endif

extern uint64_t orbit_frida_start(uint32_t, const char *, size_t);
extern void orbit_frida_stop(uint32_t, uint64_t);
typedef struct { uint32_t generation; char * name; size_t len; } Hook;
typedef struct { uint32_t generation; uint64_t handle; } Invocation;
static pthread_once_t initialized = PTHREAD_ONCE_INIT;
static void initialize(void) { gum_init_embedded(); }
void orbit_gum_init(void) { pthread_once(&initialized, initialize); }
void orbit_gum_ignore(int ignore) {
  GumInterceptor * i = gum_interceptor_obtain();
  if (ignore) gum_interceptor_ignore_current_thread(i);
  else gum_interceptor_unignore_current_thread(i);
  g_object_unref(i);
}
static void enter(GumInvocationContext * ic, gpointer data) {
  Hook * h = data;
  Invocation * call = gum_invocation_context_get_listener_invocation_data(ic, sizeof(Invocation));
  call->generation = h->generation;
  call->handle = orbit_frida_start(h->generation, h->name, h->len);
}
static void leave(GumInvocationContext * ic, gpointer data) {
  (void)data;
  Invocation * call = gum_invocation_context_get_listener_invocation_data(ic, sizeof(Invocation));
  orbit_frida_stop(call->generation, call->handle);
}
static void free_hook(gpointer data) {
  Hook * h = data;
  g_free(h->name);
  g_free(h);
}
void * orbit_gum_attach(uint64_t address, uint32_t generation, const char * name, int * status) {
  Hook * h = g_new0(Hook, 1);
  h->generation = generation;
  h->name = g_strdup(name);
  h->len = strlen(name);
  GumInvocationListener * listener = gum_make_call_listener(enter, leave, h, free_hook);
  GumInterceptor * i = gum_interceptor_obtain();
  *status = gum_interceptor_attach(i, GSIZE_TO_POINTER(address), listener, NULL);
  g_object_unref(i);
  if (*status != GUM_ATTACH_OK) { g_object_unref(listener); return NULL; }
  return listener;
}
void orbit_gum_detach(void * listener) {
  GumInterceptor * i = gum_interceptor_obtain();
  gum_interceptor_detach(i, listener);
  // Gum retains references for outstanding invocations. The agent stays loaded;
  // do not wait forever for a function that may never return.
  g_object_unref(listener);
  g_object_unref(i);
}

typedef struct { uint64_t address, offset, size; } Segment;
typedef struct { GArray * segments; } Ranges;
static gboolean add_range(const GumRangeDetails * r, gpointer data) {
  Ranges * out = data;
  if (r->file != NULL) {
    Segment s = { r->range->base_address, r->file->offset, r->range->size };
    g_array_append_val(out->segments, s);
  }
  return TRUE;
}
static GArray * segments(GumModule * module) {
  GArray * result = g_array_new(FALSE, FALSE, sizeof(Segment));
#ifdef __APPLE__
  const struct mach_header_64 * header = GSIZE_TO_POINTER(gum_module_get_range(module)->base_address);
  if (header->magic != MH_MAGIC_64 || header->ncmds > 4096 || header->sizeofcmds > 1048576) return result;
  const uint8_t * start = (const uint8_t *)(header + 1), * end = start + header->sizeofcmds;
  const struct load_command * cmd = (const void *)start;
  uint64_t text_vm = 0;
  gboolean found = FALSE;
  for (unsigned pass = 0; pass != 2; pass++) {
    cmd = (const void *)start;
    for (unsigned n = 0; n != header->ncmds; n++) {
      if ((const uint8_t *)cmd + sizeof(*cmd) > end || cmd->cmdsize < sizeof(*cmd) ||
          cmd->cmdsize > (size_t)(end - (const uint8_t *)cmd)) goto done;
      if (cmd->cmd == LC_SEGMENT_64 && cmd->cmdsize >= sizeof(struct segment_command_64)) {
        const struct segment_command_64 * seg = (const void *)cmd;
        if (pass == 0 && seg->fileoff == 0 && seg->filesize != 0) { text_vm = seg->vmaddr; found = TRUE; }
        if (pass == 1 && found && (seg->initprot & 4) && seg->filesize != 0) {
          Segment s = { (uint64_t)(uintptr_t)header + seg->vmaddr - text_vm, seg->fileoff, seg->filesize };
          g_array_append_val(result, s);
        }
      }
      cmd = (const void *)((const uint8_t *)cmd + cmd->cmdsize);
    }
  }
done:
#else
  Ranges ranges = { result };
  gum_module_enumerate_ranges(module, GUM_PAGE_EXECUTE, add_range, &ranges);
#endif
  return result;
}
typedef struct { const char * path; GumModule * module; } FindModule;
static gboolean find_module(GumModule * module, gpointer data) {
  FindModule * f = data;
  if (strcmp(gum_module_get_path(module), f->path) != 0) return TRUE;
  f->module = g_object_ref(module);
  return FALSE;
}
uint64_t orbit_gum_resolve(const char * path, uint64_t offset) {
  FindModule f = { path, NULL };
  gum_process_enumerate_modules(find_module, &f);
  if (f.module == NULL) return 0;
  GArray * ranges = segments(f.module);
  uint64_t address = 0;
  for (guint n = 0; n != ranges->len; n++) {
    Segment * s = &g_array_index(ranges, Segment, n);
    if (offset >= s->offset && offset - s->offset < s->size) { address = s->address + offset - s->offset; break; }
  }
  g_array_free(ranges, TRUE);
  g_object_unref(f.module);
  return address;
}
typedef void (* Emit)(void *, const char *, const char *, const char *, uint64_t, uint64_t, int);
typedef struct { Emit emit; void * data; GumModule * module; GArray * ranges; } Symbols;
static gboolean symbol(const GumSymbolDetails * detail, gpointer data) {
  Symbols * s = data;
  if (detail->type != GUM_SYMBOL_FUNCTION &&
      !(detail->type == GUM_SYMBOL_SECTION && detail->section != NULL &&
        g_str_has_suffix(detail->section->id, ".__text"))) return TRUE;
  for (guint n = 0; n != s->ranges->len; n++) {
    Segment * range = &g_array_index(s->ranges, Segment, n);
    if (detail->address >= range->address && detail->address - range->address < range->size) {
      s->emit(s->data, detail->name, gum_module_get_name(s->module), gum_module_get_path(s->module),
          range->offset + detail->address - range->address, detail->size > 0 ? detail->size : 0, detail->is_global);
      break;
    }
  }
  return TRUE;
}
static gboolean module_symbols(GumModule * module, gpointer data) {
  Symbols * s = data;
  s->module = module;
  s->ranges = segments(module);
  gum_module_enumerate_symbols(module, symbol, s);
  g_array_free(s->ranges, TRUE);
  return TRUE;
}
void orbit_gum_symbols(Emit emit, void * data) {
  Symbols s = { emit, data, NULL, NULL };
  gum_process_enumerate_modules(module_symbols, &s);
}
