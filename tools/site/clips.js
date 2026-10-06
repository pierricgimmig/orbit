/* Copyright (c) 2026 The Orbit Authors. All rights reserved.
   Use of this source code is governed by a BSD-style license that can be
   found in the LICENSE file. */

/* Play a clip once its stage is actually on screen. The poster holds the
   box until `playing`. Reduced motion leaves the still. A Play/Pause press
   holds the clip so scrolling it back into view does not start it again. */
(function () {
  var reduce = window.matchMedia("(prefers-reduced-motion: reduce)");
  var videos = [];

  function sync(v) {
    var btn = v._btn;
    if (!btn) return;
    var playing = !v.paused && !v.ended;
    var verb = playing ? "Pause" : "Play";
    btn.textContent = verb;
    btn.setAttribute("aria-label", verb + ": " + (v.getAttribute("aria-label") || "clip"));
  }

  function arm(v) {
    if (v.getAttribute("src")) return;
    v.muted = true;
    v.defaultMuted = true;
    v.playsInline = true;
    v.src = v.getAttribute("data-src");
  }

  function play(v) {
    arm(v);
    var pending = v.play();
    if (pending && pending.catch) pending.catch(function () { sync(v); });
  }

  function visibleEnough(el) {
    var r = el.getBoundingClientRect();
    if (r.width < 8 || r.height < 8) return false;
    var vh = window.innerHeight || document.documentElement.clientHeight;
    var shown = Math.min(r.bottom, vh) - Math.max(r.top, 0);
    return shown >= Math.min(180, r.height * 0.35);
  }

  document.querySelectorAll("figure.stage").forEach(function (fig) {
    var v = fig.querySelector("video");
    var btn = fig.querySelector(".clip-toggle");
    if (!v || !btn) return;
    v._btn = btn;
    videos.push(v);
    v.addEventListener("play", function () { sync(v); });
    v.addEventListener("playing", function () { v.classList.add("is-playing"); sync(v); });
    v.addEventListener("pause", function () { sync(v); });
    btn.addEventListener("click", function () {
      if (v.paused || v.ended) {
        delete v.dataset.hold;
        play(v);
      } else {
        v.dataset.hold = "1";
        v.pause();
      }
    });
    sync(v);
  });

  function consider(v, on) {
    var stage = v.closest("figure");
    if (!on || !visibleEnough(stage || v)) {
      if (!v.paused) v.pause();
      return;
    }
    if (reduce.matches || v.dataset.hold) return;
    play(v);
  }

  if ("IntersectionObserver" in window) {
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        var stage = e.target;
        var v = stage.querySelector("video");
        if (v) consider(v, e.isIntersecting);
      });
    }, { rootMargin: "80px 0px", threshold: [0, 0.25, 0.6] });
    document.querySelectorAll("figure.stage").forEach(function (fig) { io.observe(fig); });
  } else if (!reduce.matches) {
    videos.forEach(function (v) { if (visibleEnough(v.closest("figure") || v)) play(v); });
  }

  document.addEventListener("visibilitychange", function () {
    if (document.hidden) {
      videos.forEach(function (v) {
        if (!v.paused) { v.dataset.wasPlaying = "1"; v.pause(); }
      });
      return;
    }
    videos.forEach(function (v) {
      if (!v.dataset.wasPlaying) return;
      delete v.dataset.wasPlaying;
      if (!v.dataset.hold && !reduce.matches && visibleEnough(v)) play(v);
    });
  });

  if (reduce.addEventListener) {
    reduce.addEventListener("change", function () {
      if (reduce.matches) videos.forEach(function (v) { if (!v.paused) v.pause(); });
    });
  }
})();
