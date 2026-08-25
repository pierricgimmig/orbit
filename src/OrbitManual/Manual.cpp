// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "OrbitManual/Manual.h"

#include <absl/strings/str_cat.h>
#include <absl/strings/str_format.h>

#include <cstddef>
#include <filesystem>
#include <string>
#include <string_view>
#include <vector>

#include "OrbitBase/File.h"
#include "OrbitBase/Result.h"

namespace orbit_manual {

namespace {

// The pages are meant to be committed, so they must not pull anything from the network: no web
// fonts, no stylesheets from a CDN, no scripts. Everything the manual needs is here.
constexpr std::string_view kStyleSheet = R"CSS(:root {
  color-scheme: light dark;
  --page: #ffffff;
  --panel: #f6f7f9;
  --ink: #1c2024;
  --ink-soft: #5c6570;
  --line: #dfe3e8;
  --accent: #2f6fb5;
  --shadow: rgba(20, 26, 33, 0.12);
}

@media (prefers-color-scheme: dark) {
  :root {
    --page: #16191d;
    --panel: #1e2228;
    --ink: #e6e9ed;
    --ink-soft: #9aa4b0;
    --line: #2c323a;
    --accent: #74a9e2;
    --shadow: rgba(0, 0, 0, 0.5);
  }
}

* { box-sizing: border-box; }

body {
  margin: 0;
  padding: 0 1.5rem 5rem;
  background: var(--page);
  color: var(--ink);
  font: 16px/1.65 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial,
      sans-serif;
}

main { max-width: 60rem; margin: 0 auto; }

header.masthead {
  max-width: 60rem;
  margin: 0 auto;
  padding: 3rem 0 2rem;
  border-bottom: 1px solid var(--line);
}

header.masthead h1 { margin: 0; font-size: 2.1rem; letter-spacing: -0.02em; }
header.masthead p { margin: 0.6rem 0 0; color: var(--ink-soft); }

a { color: var(--accent); }

nav.breadcrumb { padding: 1.5rem 0 0; font-size: 0.9rem; color: var(--ink-soft); }
nav.breadcrumb a { text-decoration: none; }
nav.breadcrumb a:hover { text-decoration: underline; }

ol.contents { list-style: none; margin: 2rem 0 0; padding: 0; }

ol.contents li { margin: 0 0 0.75rem; }

ol.contents a {
  display: flex;
  gap: 1rem;
  align-items: baseline;
  padding: 1rem 1.25rem;
  border: 1px solid var(--line);
  border-radius: 10px;
  background: var(--panel);
  text-decoration: none;
  color: inherit;
}

ol.contents a:hover { border-color: var(--accent); }

ol.contents .number {
  flex: none;
  width: 2rem;
  font-variant-numeric: tabular-nums;
  color: var(--ink-soft);
}

ol.contents .title { font-weight: 600; }
ol.contents .summary { display: block; font-weight: 400; color: var(--ink-soft); margin-top: 0.2rem; }

article h2 { margin: 2.5rem 0 0.5rem; font-size: 1.7rem; letter-spacing: -0.01em; }
article .summary { margin: 0 0 1.5rem; color: var(--ink-soft); font-size: 1.05rem; }

figure {
  margin: 2rem 0;
  padding: 0;
}

figure img {
  display: block;
  width: 100%;
  height: auto;
  border: 1px solid var(--line);
  border-radius: 8px;
  box-shadow: 0 2px 12px var(--shadow);
  background: var(--panel);
}

figcaption { margin-top: 0.75rem; color: var(--ink-soft); font-size: 0.92rem; }

footer.pager {
  display: flex;
  justify-content: space-between;
  gap: 1rem;
  margin-top: 3rem;
  padding-top: 1.5rem;
  border-top: 1px solid var(--line);
  font-size: 0.95rem;
}

footer.pager a { text-decoration: none; }
footer.pager a:hover { text-decoration: underline; }

p.generated { margin-top: 3rem; color: var(--ink-soft); font-size: 0.85rem; }
)CSS";

[[nodiscard]] std::string PageHead(std::string_view title, std::string_view style_sheet_path) {
  return absl::StrFormat(
      "<!DOCTYPE html>\n"
      "<html lang=\"en\">\n"
      "<head>\n"
      "<meta charset=\"utf-8\">\n"
      "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n"
      "<title>%s</title>\n"
      "<link rel=\"stylesheet\" href=\"%s\">\n"
      "</head>\n"
      "<body>\n",
      EscapeHtml(title), style_sheet_path);
}

[[nodiscard]] std::string ChapterFileName(const Chapter& chapter) {
  return absl::StrCat(chapter.id, ".html");
}

[[nodiscard]] ErrorMessageOr<void> WriteFile(const std::filesystem::path& path,
                                             std::string_view content) {
  OUTCOME_TRY(auto&& fd, orbit_base::OpenFileForWriting(path));
  OUTCOME_TRY(orbit_base::WriteFully(fd, content));
  return outcome::success();
}

}  // namespace

std::string EscapeHtml(std::string_view text) {
  std::string result;
  result.reserve(text.size());
  for (char character : text) {
    switch (character) {
      case '&':
        result.append("&amp;");
        break;
      case '<':
        result.append("&lt;");
        break;
      case '>':
        result.append("&gt;");
        break;
      case '"':
        result.append("&quot;");
        break;
      case '\'':
        result.append("&#39;");
        break;
      default:
        result.push_back(character);
    }
  }
  return result;
}

ErrorMessageOr<void> Manual::WriteStyleSheet() const {
  return WriteFile(output_directory_ / "style.css", kStyleSheet);
}

ErrorMessageOr<void> Manual::WriteIndex() const {
  std::string page = PageHead("Orbit Manual", "style.css");
  absl::StrAppend(&page,
                  "<header class=\"masthead\">\n<h1>Orbit Manual</h1>\n<p>",
                  EscapeHtml(subtitle_), "</p>\n</header>\n<main>\n<ol class=\"contents\">\n");

  for (size_t index = 0; index < chapters_.size(); ++index) {
    const Chapter& chapter = chapters_[index];
    absl::StrAppendFormat(&page,
                          "<li><a href=\"%s\"><span class=\"number\">%d</span><span "
                          "class=\"title\">%s<span class=\"summary\">%s</span></span></a></li>\n",
                          ChapterFileName(chapter), index + 1, EscapeHtml(chapter.title),
                          EscapeHtml(chapter.summary));
  }

  absl::StrAppend(&page, "</ol>\n</main>\n</body>\n</html>\n");
  return WriteFile(output_directory_ / "index.html", page);
}

ErrorMessageOr<void> Manual::WriteChapter(size_t index) const {
  const Chapter& chapter = chapters_[index];

  std::string page = PageHead(absl::StrCat(chapter.title, " - Orbit Manual"), "style.css");
  absl::StrAppend(&page,
                  "<main>\n<nav class=\"breadcrumb\"><a href=\"index.html\">Orbit "
                  "Manual</a></nav>\n<article>\n");
  absl::StrAppendFormat(&page, "<h2>%s</h2>\n<p class=\"summary\">%s</p>\n",
                        EscapeHtml(chapter.title), EscapeHtml(chapter.summary));

  for (const std::string& paragraph : chapter.paragraphs) {
    absl::StrAppendFormat(&page, "<p>%s</p>\n", EscapeHtml(paragraph));
  }

  for (const Screenshot& screenshot : chapter.screenshots) {
    absl::StrAppendFormat(&page,
                          "<figure>\n<img src=\"images/%s\" alt=\"%s\" "
                          "loading=\"lazy\">\n<figcaption>%s</figcaption>\n</figure>\n",
                          screenshot.file_name, EscapeHtml(screenshot.caption),
                          EscapeHtml(screenshot.caption));
  }

  absl::StrAppend(&page, "</article>\n<footer class=\"pager\">\n");
  if (index > 0) {
    absl::StrAppendFormat(&page, "<a href=\"%s\">&larr; %s</a>\n",
                          ChapterFileName(chapters_[index - 1]),
                          EscapeHtml(chapters_[index - 1].title));
  } else {
    absl::StrAppend(&page, "<span></span>\n");
  }
  if (index + 1 < chapters_.size()) {
    absl::StrAppendFormat(&page, "<a href=\"%s\">%s &rarr;</a>\n",
                          ChapterFileName(chapters_[index + 1]),
                          EscapeHtml(chapters_[index + 1].title));
  } else {
    absl::StrAppend(&page, "<span></span>\n");
  }
  absl::StrAppend(&page, "</footer>\n</main>\n</body>\n</html>\n");

  return WriteFile(output_directory_ / ChapterFileName(chapter), page);
}

ErrorMessageOr<void> Manual::Write() const {
  OUTCOME_TRY(orbit_base::CreateDirectories(GetImageDirectory()));
  OUTCOME_TRY(WriteStyleSheet());
  OUTCOME_TRY(WriteIndex());
  for (size_t index = 0; index < chapters_.size(); ++index) {
    OUTCOME_TRY(WriteChapter(index));
  }
  return outcome::success();
}

}  // namespace orbit_manual
