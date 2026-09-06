// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// The workload the end-to-end suite profiles: Box3D falling boxes, one world
// per thread, stepped until told to stop.
//
// Why a purpose-built driver rather than Box3D's own `test` or `benchmark`
// binaries: a profiler target has to be long-lived (those exit in seconds,
// and a capture cannot follow a pid that is gone), multi-threaded (so the
// screenshots show more than one track), and busy in named functions the
// symbolizer can resolve. Each thread owning a world gives all three without
// needing Box3D's task-system callbacks.

#define _GNU_SOURCE  /* pthread_setname_np */

#include "box3d/box3d.h"
#include "box3d/collision.h"
#include "box3d/types.h"

#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static atomic_int g_running = 1;

// Box3D keeps worlds in a process-global table, so creating and destroying
// them is not thread safe even when each thread only touches its own world.
// Stepping is: that is per-world state. Three threads creating worlds at once
// crashed with SIGILL until this lock existed, which is exactly the kind of
// thing the end-to-end suite is meant to surface early.
static pthread_mutex_t g_world_lock = PTHREAD_MUTEX_INITIALIZER;

static b3WorldId create_world_locked(const b3WorldDef* def) {
  pthread_mutex_lock(&g_world_lock);
  b3WorldId world = b3CreateWorld(def);
  pthread_mutex_unlock(&g_world_lock);
  return world;
}

static void destroy_world_locked(b3WorldId world) {
  pthread_mutex_lock(&g_world_lock);
  b3DestroyWorld(world);
  pthread_mutex_unlock(&g_world_lock);
}

static void on_signal(int signum) {
  (void)signum;
  atomic_store(&g_running, 0);
}

// Named so they are recognisable in a flame graph and in the hook picker.
// noinline: these exist to be found by the profiler. At -O2 a one-line
// static function is inlined away, and a function that is not in the binary
// cannot be sampled, searched for, or hooked.
__attribute__((noinline)) static void orbit_e2e_build_world(b3WorldId world, int boxes_per_side) {
  b3BodyDef ground_def = b3DefaultBodyDef();
  b3ShapeDef shape_def = b3DefaultShapeDef();
  b3BodyId ground = b3CreateBody(world, &ground_def);
  b3BoxHull ground_box = b3MakeBoxHull(50.0f, 1.0f, 50.0f);
  b3CreateHullShape(ground, &shape_def, &ground_box.base);

  b3BodyDef body_def = b3DefaultBodyDef();
  body_def.type = b3_dynamicBody;
  b3BoxHull cube = b3MakeCubeHull(0.5f);
  for (int i = 0; i < boxes_per_side; ++i) {
    for (int j = 0; j < boxes_per_side; ++j) {
      for (int k = 0; k < boxes_per_side; ++k) {
        body_def.position = (b3Vec3){-8.0f + 2.0f * (float)j,
                                     3.0f + 2.0f * (float)i,
                                     -8.0f + 2.0f * (float)k};
        b3BodyId body = b3CreateBody(world, &body_def);
        b3CreateHullShape(body, &shape_def, &cube.base);
      }
    }
  }
}

__attribute__((noinline)) static void orbit_e2e_step_world(b3WorldId world) {
  b3World_Step(world, 1.0f / 60.0f, 4);
}

typedef struct {
  int index;
  int boxes_per_side;
} WorkerArgs;

static void* orbit_e2e_worker(void* raw) {
  WorkerArgs* args = raw;
  char name[16];
  snprintf(name, sizeof(name), "physics-%d", args->index);
  pthread_setname_np(pthread_self(), name);

  b3WorldDef world_def = b3DefaultWorldDef();
  world_def.workerCount = 1;  // No task callbacks: this thread is the worker.
  b3WorldId world = create_world_locked(&world_def);
  orbit_e2e_build_world(world, args->boxes_per_side);

  long long steps = 0;
  while (atomic_load(&g_running)) {
    orbit_e2e_step_world(world);
    // Restart once the stack has settled, so the workload never goes quiet
    // and a capture taken at any moment looks the same.
    if (++steps % 900 == 0) {
      destroy_world_locked(world);
      world = create_world_locked(&world_def);
      orbit_e2e_build_world(world, args->boxes_per_side);
    }
  }
  destroy_world_locked(world);
  return NULL;
}

int main(int argc, char** argv) {
  int threads = 3;
  int boxes_per_side = 6;
  for (int i = 1; i < argc; ++i) {
    if (strcmp(argv[i], "--threads") == 0 && i + 1 < argc) {
      threads = atoi(argv[++i]);
    } else if (strcmp(argv[i], "--boxes") == 0 && i + 1 < argc) {
      boxes_per_side = atoi(argv[++i]);
    }
  }
  if (threads < 1) threads = 1;

  signal(SIGTERM, on_signal);
  signal(SIGINT, on_signal);

  // The harness reads this to know which pid to profile.
  printf("orbit-e2e-target pid=%d threads=%d\n", (int)getpid(), threads);
  fflush(stdout);

  pthread_t* ids = calloc((size_t)threads, sizeof(pthread_t));
  WorkerArgs* args = calloc((size_t)threads, sizeof(WorkerArgs));
  for (int i = 0; i < threads; ++i) {
    args[i].index = i;
    args[i].boxes_per_side = boxes_per_side;
    pthread_create(&ids[i], NULL, orbit_e2e_worker, &args[i]);
  }
  for (int i = 0; i < threads; ++i) {
    pthread_join(ids[i], NULL);
  }
  free(ids);
  free(args);
  return 0;
}
