// Dummy target for Orbit auto-profiling (PR #97). Deterministic, frame-paced
// "game loop" with a nested call tree and functions at very different rates.
// Build: gcc -O0 -g -fno-omit-frame-pointer -fno-inline -o game_loop game_loop.c
//
// Per second (at 60 frames/s):
//   run_frame        60/s   ~12 ms busy per frame (+ sleep to the 16.7 ms tick)
//   simulate_physics 60/s   ~4 ms   -> calls integrate_body 16x = 960/s   (too hot for a 1000/s budget: > 500/s)
//   update_ai        60/s   ~3 ms
//   render_scene     60/s   ~4 ms   -> calls draw_mesh 8x  = 480/s        (under the 500/s "too hot" line)
//   hash_mix         ~200k/s  tiny leaf called in tight loops by update_ai (flood; auto-unhook / too hot)
//   stream_assets    1x every 2 s, 150 ms busy (silence window of 1.5 s -> may be judged "silent")
//   phase2_pathfind  only after PHASE2_AT_S seconds: a new code path, 60/s, ~3 ms
//   main_loop        entered once, never returns (hooked -> "silent")
// Some functions start with `call` right after the frame setup (dispatch_* ),
// which the hook-safety analyzer rates "risky" for Frida; uprobes are fine.
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>

static volatile uint64_t sink;
static int PHASE2_AT_S = 40;

static uint64_t now_ns(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (uint64_t)ts.tv_sec * 1000000000ull + ts.tv_nsec;
}

// Spin for about `us` microseconds doing arithmetic (shows up in samples).
__attribute__((noinline)) void burn_us(uint64_t us) {
  uint64_t end = now_ns() + us * 1000ull, x = sink;
  while (now_ns() < end) {
    for (int i = 0; i < 200; i++) x = x * 6364136223846793005ull + 1442695040888963407ull;
  }
  sink = x;
}

__attribute__((noinline)) uint64_t hash_mix(uint64_t v) {
  uint64_t local = v ^ 0x9e3779b97f4a7c15ull;
  local ^= local >> 33;
  local *= 0xff51afd7ed558ccdull;
  local ^= local >> 33;
  return local;
}

__attribute__((noinline)) void integrate_body(int i) {
  int local = i * 3;
  burn_us(200);  // 16 bodies * 200 us = 3.2 ms per frame
  sink += local;
}

__attribute__((noinline)) void simulate_physics(void) {
  int bodies = 16;
  for (int i = 0; i < bodies; i++) integrate_body(i);
  burn_us(800);
}

__attribute__((noinline)) void update_ai(void) {
  int agents = 3;
  uint64_t end = now_ns() + 3000000ull;  // ~3 ms of hashing
  uint64_t h = sink;
  while (now_ns() < end) {
    for (int k = 0; k < agents; k++) h = hash_mix(h + k);
  }
  sink = h;
}

__attribute__((noinline)) void draw_mesh(int m) {
  int local = m + 1;
  burn_us(400);  // 8 meshes * 400 us = 3.2 ms
  sink += local;
}

__attribute__((noinline)) void render_scene(void) {
  int meshes = 8;
  for (int m = 0; m < meshes; m++) draw_mesh(m);
  burn_us(600);
}

__attribute__((noinline)) void phase2_pathfind(void) {
  int nodes = 64;
  burn_us(3000);
  sink += nodes;
}

__attribute__((noinline)) void stream_assets(void) {
  int chunks = 10;
  burn_us(150000);
  sink += chunks;
}

// Pure dispatcher: `push rbp; mov rbp,rsp; call ...` -> a call in the first
// 5 bytes: "risky" to the analyzer (a Frida relocation hazard only).
__attribute__((noinline)) void dispatch_game_systems(void) {
  simulate_physics();
  update_ai();
}

__attribute__((noinline)) void run_frame(uint64_t frame, int phase2) {
  uint64_t local = frame;
  dispatch_game_systems();
  render_scene();
  if (phase2) phase2_pathfind();
  sink += local;
}

__attribute__((noinline)) void main_loop(void) {
  const uint64_t tick = 16666667ull;
  uint64_t start = now_ns(), next = start, frame = 0, last_stream = start;
  for (;;) {
    int phase2 = (now_ns() - start) > (uint64_t)PHASE2_AT_S * 1000000000ull;
    run_frame(frame++, phase2);
    if (now_ns() - last_stream >= 2000000000ull) {
      stream_assets();
      last_stream = now_ns();
    }
    next += tick;
    uint64_t t = now_ns();
    if (next > t) {
      struct timespec d = {0, (long)(next - t)};
      nanosleep(&d, NULL);
    } else {
      next = t;  // fell behind; don't spiral
    }
    if (frame % 600 == 0) { printf("frame %llu\n", (unsigned long long)frame); fflush(stdout); }
  }
}

int main(int argc, char** argv) {
  if (argc > 1) PHASE2_AT_S = atoi(argv[1]);
  printf("game_loop pid %d, phase2 at %d s\n", getpid(), PHASE2_AT_S);
  fflush(stdout);
  main_loop();
  return 0;
}
