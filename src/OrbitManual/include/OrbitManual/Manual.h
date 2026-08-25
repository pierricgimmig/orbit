// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_MANUAL_MANUAL_H_
#define ORBIT_MANUAL_MANUAL_H_

#include <filesystem>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include "OrbitBase/Result.h"

namespace orbit_manual {

// One image, with the caption that goes underneath it. `file_name` is relative to the manual's
// image directory, so that a page can refer to it as "images/<file_name>".
struct Screenshot {
  std::string file_name;
  std::string caption;
};

// One page of the manual: what a feature is for, how to reach it, and what it looks like.
//
// A chapter is written by the code that drives the UI, which is the only place that knows what
// ended up on screen. Prose that does not describe a screenshot belongs in `paragraphs`; prose
// that describes one belongs in that screenshot's caption.
struct Chapter {
  // Used for the page's file name and for its anchor on the index, so it has to be URL-safe.
  std::string id;
  std::string title;
  // One sentence. This is what the index shows, so it should read on its own.
  std::string summary;
  std::vector<std::string> paragraphs;
  std::vector<Screenshot> screenshots;
};

// Collects chapters and writes them out as a small static site: an index, one page per chapter,
// and a stylesheet. Nothing is loaded from the network, so the result works from a file:// URL and
// from GitHub Pages alike.
class Manual {
 public:
  explicit Manual(std::filesystem::path output_directory)
      : output_directory_(std::move(output_directory)) {}

  // Where a chapter's screenshots have to be written for the generated pages to find them.
  [[nodiscard]] std::filesystem::path GetImageDirectory() const {
    return output_directory_ / "images";
  }

  void SetSubtitle(std::string subtitle) { subtitle_ = std::move(subtitle); }
  void AddChapter(Chapter chapter) { chapters_.push_back(std::move(chapter)); }
  [[nodiscard]] const std::vector<Chapter>& GetChapters() const { return chapters_; }

  // Creates the output directory if it does not exist and writes every page.
  [[nodiscard]] ErrorMessageOr<void> Write() const;

 private:
  [[nodiscard]] ErrorMessageOr<void> WriteIndex() const;
  [[nodiscard]] ErrorMessageOr<void> WriteChapter(size_t index) const;
  [[nodiscard]] ErrorMessageOr<void> WriteStyleSheet() const;

  std::filesystem::path output_directory_;
  std::string subtitle_;
  std::vector<Chapter> chapters_;
};

// Escapes the five characters that cannot appear literally in HTML text or in an attribute.
[[nodiscard]] std::string EscapeHtml(std::string_view text);

}  // namespace orbit_manual

#endif  // ORBIT_MANUAL_MANUAL_H_
