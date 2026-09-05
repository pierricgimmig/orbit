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
  --chrome: #ffffff;
  --ink: #202124;
  --ink-soft: #5f6368;
  --line: #dadce0;
  --hover: #f8f9fa;
  --accent: #1a73e8;
  --accent-strong: #174ea6;
  --accent-soft: #e8f0fe;
  --shadow: 0 1px 2px rgba(60, 64, 67, 0.18), 0 2px 8px rgba(60, 64, 67, 0.1);
  --shadow-soft: 0 1px 2px rgba(60, 64, 67, 0.08);
  --prose: 46rem;
  --wide: 54rem;
  --gutter: 1.5rem;
  --nav-height: 3.5rem;
}

@media (prefers-color-scheme: dark) {
  :root {
    --page: #202124;
    --chrome: #292a2d;
    --ink: #e8eaed;
    --ink-soft: #9aa0a6;
    --line: #3c4043;
    --hover: #303134;
    --accent: #8ab4f8;
    --accent-strong: #aecbfa;
    --accent-soft: #174ea6;
    --shadow: 0 1px 2px rgba(0, 0, 0, 0.4), 0 4px 16px rgba(0, 0, 0, 0.28);
    --shadow-soft: 0 1px 2px rgba(0, 0, 0, 0.28);
  }
}

* { box-sizing: border-box; }

html {
  scroll-padding-top: calc(var(--nav-height) + 1rem);
}

body {
  margin: 0;
  min-height: 100vh;
  background: var(--page);
  color: var(--ink);
  font: 16px/1.7 "Roboto", "Segoe UI", system-ui, -apple-system, BlinkMacSystemFont,
      "Helvetica Neue", Arial, sans-serif;
  letter-spacing: 0.01em;
  text-rendering: optimizeLegibility;
}

a {
  color: var(--accent);
  text-decoration: none;
}

a:hover { text-decoration: underline; }

a:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 3px;
  border-radius: 4px;
}

.topbar {
  position: sticky;
  top: 0;
  z-index: 10;
  height: var(--nav-height);
  background: var(--chrome);
  border-bottom: 1px solid var(--line);
  box-shadow: var(--shadow-soft);
}

.topbar-inner {
  display: flex;
  align-items: center;
  max-width: var(--wide);
  height: 100%;
  margin: 0 auto;
  padding: 0 var(--gutter);
}

.brand {
  display: inline-flex;
  align-items: baseline;
  gap: 0.45rem;
  color: var(--ink);
  font-size: 1.125rem;
  font-weight: 500;
  letter-spacing: -0.02em;
  text-decoration: none;
}

.brand:hover { color: var(--ink); text-decoration: none; }

.brand span {
  font-weight: 400;
  color: var(--ink-soft);
}

main {
  max-width: var(--wide);
  margin: 0 auto;
  padding: 0 var(--gutter) 5rem;
}

.hero {
  max-width: var(--prose);
  padding: 4.5rem 0 2.25rem;
}

.eyebrow {
  margin: 0 0 0.75rem;
  color: var(--accent);
  font-size: 0.8125rem;
  font-weight: 500;
  letter-spacing: 0.06em;
  text-transform: uppercase;
}

.hero h1 {
  margin: 0;
  font-size: 2.75rem;
  font-weight: 400;
  line-height: 1.15;
  letter-spacing: -0.03em;
}

.lede {
  margin: 1rem 0 0;
  max-width: 40rem;
  color: var(--ink-soft);
  font-size: 1.0625rem;
  line-height: 1.65;
}

.toc { padding-top: 0.5rem; }

.toc > h2,
.section-label {
  margin: 0 0 0.75rem;
  color: var(--ink-soft);
  font-size: 0.75rem;
  font-weight: 500;
  letter-spacing: 0.08em;
  text-transform: uppercase;
}

ol.contents {
  list-style: none;
  margin: 0;
  padding: 0;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: var(--chrome);
  overflow: hidden;
}

ol.contents li { margin: 0; }

ol.contents li + li { border-top: 1px solid var(--line); }

ol.contents a {
  display: grid;
  grid-template-columns: 2.5rem minmax(0, 1fr);
  gap: 0.25rem 1rem;
  align-items: start;
  padding: 1rem 1.25rem;
  color: inherit;
  text-decoration: none;
}

ol.contents a:hover {
  background: var(--hover);
  text-decoration: none;
}

ol.contents a:hover .title { color: var(--accent); }

ol.contents .number {
  grid-row: 1 / span 2;
  padding-top: 0.1rem;
  color: var(--ink-soft);
  font-size: 0.875rem;
  font-variant-numeric: tabular-nums;
  letter-spacing: 0;
}

ol.contents .title {
  font-size: 1rem;
  font-weight: 500;
  letter-spacing: -0.01em;
}

ol.contents .summary {
  display: block;
  margin-top: 0.15rem;
  color: var(--ink-soft);
  font-size: 0.875rem;
  font-weight: 400;
  line-height: 1.5;
}

nav.breadcrumb {
  padding: 1.75rem 0 0;
  color: var(--ink-soft);
  font-size: 0.8125rem;
}

nav.breadcrumb a { font-weight: 500; }

nav.breadcrumb a:hover { text-decoration: underline; }

article { padding-top: 0.35rem; }

article h2 {
  max-width: var(--prose);
  margin: 0.35rem 0 0.65rem;
  font-size: 2.125rem;
  font-weight: 400;
  line-height: 1.2;
  letter-spacing: -0.025em;
}

article > p,
article .summary {
  max-width: var(--prose);
}

article .summary {
  margin: 0 0 1.75rem;
  color: var(--ink-soft);
  font-size: 1.125rem;
  line-height: 1.55;
}

article p {
  margin: 0 0 1.15rem;
}

article p:last-of-type { margin-bottom: 0; }

figure {
  margin: 2.25rem 0 0;
  padding: 0;
}

figure img {
  display: block;
  width: 100%;
  height: auto;
  border: 0;
  border-radius: 8px;
  background: var(--hover);
  box-shadow: var(--shadow);
}

figcaption {
  max-width: var(--prose);
  margin-top: 0.85rem;
  color: var(--ink-soft);
  font-size: 0.8125rem;
  line-height: 1.5;
}

footer.pager {
  display: flex;
  justify-content: space-between;
  gap: 1.5rem;
  max-width: var(--prose);
  margin-top: 3.5rem;
  padding-top: 1.5rem;
  border-top: 1px solid var(--line);
}

footer.pager a {
  display: flex;
  flex-direction: column;
  gap: 0.15rem;
  max-width: 48%;
  font-weight: 500;
  text-decoration: none;
}

footer.pager a:hover { text-decoration: none; }

footer.pager a:hover .pager-title { text-decoration: underline; }

footer.pager .next { align-items: flex-end; text-align: right; }

footer.pager .dir {
  color: var(--ink-soft);
  font-size: 0.75rem;
  font-weight: 400;
  letter-spacing: 0.04em;
  text-transform: uppercase;
}

footer.pager .pager-title { color: var(--accent); }

p.generated {
  margin-top: 3rem;
  color: var(--ink-soft);
  font-size: 0.8125rem;
}

@media (max-width: 40rem) {
  :root { --gutter: 1.15rem; }

  .hero { padding: 2.75rem 0 1.5rem; }

  .hero h1 { font-size: 2.1rem; }

  article h2 { font-size: 1.75rem; }

  ol.contents a {
    grid-template-columns: 1.75rem minmax(0, 1fr);
    padding: 0.9rem 1rem;
  }

  footer.pager { flex-direction: column; }

  footer.pager a,
  footer.pager .next { max-width: 100%; align-items: flex-start; text-align: left; }
}
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
      "<body>\n"
      "<header class=\"topbar\">\n"
      "<div class=\"topbar-inner\">\n"
      "<a class=\"brand\" href=\"index.html\">Orbit<span>Manual</span></a>\n"
      "</div>\n"
      "</header>\n",
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
                  "<main>\n<header class=\"hero\">\n<p class=\"eyebrow\">Documentation</p>\n"
                  "<h1>Orbit Manual</h1>\n<p class=\"lede\">",
                  EscapeHtml(subtitle_),
                  "</p>\n</header>\n<section class=\"toc\" aria-labelledby=\"contents-heading\">\n"
                  "<h2 id=\"contents-heading\">Contents</h2>\n<ol class=\"contents\">\n");

  for (size_t index = 0; index < chapters_.size(); ++index) {
    const Chapter& chapter = chapters_[index];
    absl::StrAppendFormat(&page,
                          "<li><a href=\"%s\"><span class=\"number\">%d</span><span "
                          "class=\"title\">%s<span class=\"summary\">%s</span></span></a></li>\n",
                          ChapterFileName(chapter), index + 1, EscapeHtml(chapter.title),
                          EscapeHtml(chapter.summary));
  }

  absl::StrAppend(&page, "</ol>\n</section>\n</main>\n</body>\n</html>\n");
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
    absl::StrAppendFormat(&page,
                          "<a class=\"prev\" href=\"%s\"><span class=\"dir\">Previous</span>"
                          "<span class=\"pager-title\">%s</span></a>\n",
                          ChapterFileName(chapters_[index - 1]),
                          EscapeHtml(chapters_[index - 1].title));
  } else {
    absl::StrAppend(&page, "<span></span>\n");
  }
  if (index + 1 < chapters_.size()) {
    absl::StrAppendFormat(&page,
                          "<a class=\"next\" href=\"%s\"><span class=\"dir\">Next</span>"
                          "<span class=\"pager-title\">%s</span></a>\n",
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
