// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "OrbitManual/Chapters.h"

#include <absl/strings/str_cat.h>
#include <absl/time/time.h>

#include <QAbstractItemModel>
#include <QAction>
#include <QApplication>
#include <QDialog>
#include <QLineEdit>
#include <QMessageBox>
#include <QHeaderView>
#include <QItemSelectionModel>
#include <QPushButton>
#include <QSize>
#include <QSplitter>
#include <QString>
#include <QTabWidget>
#include <QTimer>
#include <QTreeView>
#include <QWidget>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include "OrbitBase/Logging.h"
#include "OrbitBase/Result.h"
#include "OrbitManual/Manual.h"
#include "OrbitManual/Screenshots.h"
#include "OrbitQt/orbitmainwindow.h"

namespace orbit_manual {

namespace {

// Dialogs are opened at least this large before being screenshotted; see RecordModalDialog.
constexpr QSize kMinimumDialogSize{1000, 850};

// Walks the UI, screenshotting as it goes. One method per chapter, called in order by Record().
class Recorder {
 public:
  Recorder(OrbitMainWindow* window, Manual* manual, const RecordingOptions& options)
      : window_(window),
        manual_(manual),
        options_(options),
        screenshotter_(manual->GetImageDirectory()) {}

  [[nodiscard]] ErrorMessageOr<void> Record();

 private:
  // Waits for the target's symbols, which the first screenshot already depends on: a window whose
  // module list still says "Loading..." is not what the manual is trying to show.
  void WaitForSymbols();

  // Chapters, in the order a reader meets them.
  void RecordMainWindow();
  void RecordSymbols();
  void RecordCaptureOptions();
  [[nodiscard]] ErrorMessageOr<void> RecordTakingACapture();
  void RecordCaptureWindow();
  void RecordTrackConfiguration();
  void RecordLiveFunctions();
  void RecordManualInstrumentation();
  void RecordSampling();
  void RecordCallTrees();
  void RecordCaptureLog();
  void RecordSymbolLocations();
  void RecordAbout();

  // UI lookup. Every one of these fails loudly rather than returning something unusable: a name
  // that no longer resolves means the manual would silently lose a chapter.
  [[nodiscard]] QAction* GetAction(std::string_view name) const;
  [[nodiscard]] QWidget* GetWidget(std::string_view name) const;
  [[nodiscard]] QTabWidget* GetRightTabs() const;

  // Brings `tab` to the front of the tab widget it lives in and lets the switch settle.
  void ShowRightTab(std::string_view tab_name);
  [[nodiscard]] bool IsRightTabEnabled(std::string_view tab_name) const;

  // Types `text` into the filter of the data view panel called `panel_name`.
  void SetFilter(std::string_view panel_name, std::string_view filter_line_edit, const QString& text);

  // Widens every column of every tree inside `parent` to fit what is in it. Orbit's columns keep
  // whatever width they were given by the layout, which on a screenshot turns numbers into
  // ellipses.
  void FitColumns(QWidget* parent);

  // Gives the capture window `share` of the window's width, the analysis tabs the rest.
  void SetCaptureWindowShare(double share);

  // Selects the first row of the tree inside `parent`, the way a reader would click it.
  void SelectFirstRow(QWidget* parent, std::string_view tree_name);

  // Adds a screenshot of `widget` to `chapter`, or logs why it could not be taken. A missing
  // screenshot leaves a readable chapter behind, so this never fails the run.
  void AddScreenshot(Chapter* chapter, QWidget* widget, std::string_view name, std::string caption);

  // Triggers `action`, which opens a modal dialog, screenshots the dialog and closes it again.
  // Qt's exec() does not return until the dialog closes, so the closing has to be queued first.
  void RecordModalDialog(Chapter* chapter, std::string_view action_name, std::string_view name,
                         std::string caption);

  OrbitMainWindow* window_;
  Manual* manual_;
  RecordingOptions options_;
  Screenshotter screenshotter_;
  // Set while a chapter is deliberately showing a modal dialog, so that the watchdog that closes
  // unexpected ones leaves it alone.
  bool expecting_modal_dialog_ = false;
};

QAction* Recorder::GetAction(std::string_view name) const {
  QAction* action = window_->findChild<QAction*>(QString::fromUtf8(name.data(), name.size()));
  ORBIT_CHECK(action != nullptr);
  return action;
}

QWidget* Recorder::GetWidget(std::string_view name) const {
  QWidget* widget = window_->findChild<QWidget*>(QString::fromUtf8(name.data(), name.size()));
  ORBIT_CHECK(widget != nullptr);
  return widget;
}

QTabWidget* Recorder::GetRightTabs() const {
  auto* tabs = window_->findChild<QTabWidget*>("RightTabWidget");
  ORBIT_CHECK(tabs != nullptr);
  return tabs;
}

bool Recorder::IsRightTabEnabled(std::string_view tab_name) const {
  QTabWidget* tabs = GetRightTabs();
  return tabs->isTabEnabled(tabs->indexOf(GetWidget(tab_name)));
}

void Recorder::ShowRightTab(std::string_view tab_name) {
  QTabWidget* tabs = GetRightTabs();
  QWidget* tab = GetWidget(tab_name);
  if (!tabs->isTabEnabled(tabs->indexOf(tab))) {
    ORBIT_ERROR("Tab \"%s\" is disabled; the manual will be missing what it shows",
                std::string{tab_name});
    return;
  }
  tabs->setCurrentWidget(tab);
  PumpEventsFor(absl::Milliseconds(500));
}

void Recorder::SetFilter(std::string_view panel_name, std::string_view filter_line_edit,
                         const QString& text) {
  QWidget* panel = GetWidget(panel_name);
  auto* filter =
      panel->findChild<QLineEdit*>(QString::fromUtf8(filter_line_edit.data(), filter_line_edit.size()));
  ORBIT_CHECK(filter != nullptr);
  filter->setText(text);
  PumpEventsFor(absl::Milliseconds(750));
}

void Recorder::FitColumns(QWidget* parent) {
  for (QTreeView* tree : parent->findChildren<QTreeView*>()) {
    if (tree->model() == nullptr) continue;
    tree->header()->resizeSections(QHeaderView::ResizeToContents);
  }
  PumpEventsFor(absl::Milliseconds(250));
}

void Recorder::SetCaptureWindowShare(double share) {
  auto* splitter = window_->findChild<QSplitter*>("splitter_2");
  ORBIT_CHECK(splitter != nullptr);
  const int total = splitter->width();
  splitter->setSizes({static_cast<int>(total * share), static_cast<int>(total * (1.0 - share))});
  // The capture window paints through OpenGL and only repaints when it decides it needs to, so
  // give it time to notice the new size before anything grabs its framebuffer.
  PumpEventsFor(absl::Seconds(2));
  ORBIT_LOG("Capture window is now %d of %d pixels wide", splitter->sizes().front(), total);
}

void Recorder::SelectFirstRow(QWidget* parent, std::string_view tree_name) {
  auto* tree =
      parent->findChild<QTreeView*>(QString::fromUtf8(tree_name.data(), tree_name.size()));
  if (tree == nullptr || tree->model() == nullptr || tree->model()->rowCount() == 0) {
    ORBIT_ERROR("Nothing to select in \"%s\"", std::string{tree_name});
    return;
  }
  tree->setCurrentIndex(tree->model()->index(0, 0));
  PumpEventsFor(absl::Seconds(1));
}

void Recorder::AddScreenshot(Chapter* chapter, QWidget* widget, std::string_view name,
                             std::string caption) {
  // Deliberately does not pump the event loop: this is also called from inside a modal dialog's
  // event loop, and re-entering the loop from there has been enough to trip assertions deep in
  // Orbit's data structures. Callers do the waiting before they call this.
  ErrorMessageOr<std::string> file_name_or_error = screenshotter_.Grab(widget, name);
  if (file_name_or_error.has_error()) {
    ORBIT_ERROR("Screenshot \"%s\" was not taken: %s", std::string{name},
                file_name_or_error.error().message());
    return;
  }
  chapter->screenshots.push_back(
      Screenshot{std::move(file_name_or_error.value()), std::move(caption)});
}

void Recorder::RecordModalDialog(Chapter* chapter, std::string_view action_name,
                                 std::string_view name, std::string caption) {
  expecting_modal_dialog_ = true;

  // The dialog's own event loop runs this, which is the only chance to grab it: trigger() below
  // does not return until the dialog is gone.
  QTimer::singleShot(1500, window_, [this, chapter, name, caption]() mutable {
    QWidget* dialog = QApplication::activeModalWidget();
    if (dialog == nullptr) {
      ORBIT_ERROR("No modal dialog appeared for \"%s\"", std::string{name});
      return;
    }
    // Orbit's dialogs open at whatever size their layout settles on, which for the ones with a
    // scroll area is small enough to cut sentences in half. A screenshot of a dialog is only
    // useful if the whole dialog is in it.
    dialog->resize(dialog->sizeHint().expandedTo(kMinimumDialogSize));
    QApplication::processEvents();

    AddScreenshot(chapter, dialog, name, std::move(caption));
    dialog->close();
  });

  GetAction(action_name)->trigger();
  expecting_modal_dialog_ = false;
  PumpEventsFor(absl::Milliseconds(500));
}

void Recorder::WaitForSymbols() {
  auto* tree = GetWidget("FunctionsList")->findChild<QTreeView*>("treeView");
  ORBIT_CHECK(tree != nullptr);
  const bool have_functions = PumpEventsUntil(
      [tree]() { return tree->model() != nullptr && tree->model()->rowCount() > 0; },
      options_.symbol_timeout);
  if (!have_functions) {
    ORBIT_ERROR("No functions were listed within %s; symbols may be missing",
                absl::FormatDuration(options_.symbol_timeout));
  }
}

void Recorder::RecordMainWindow() {
  Chapter chapter;
  chapter.id = "main-window";
  chapter.title = "The main window";
  chapter.summary =
      "Where everything else in this manual is reached from: the capture window on the left, the "
      "analysis tabs on the right.";
  chapter.paragraphs = {
      "Orbit opens on a target it is already connected to. The left half is the capture window, "
      "which is empty until a capture has been taken. The right half is where a capture is taken "
      "apart afterwards, one tab per way of looking at it.",
      "The toolbar above the capture window holds the capture button, the two filter boxes and "
      "the capture timer. The title bar names the process being profiled.",
  };
  AddScreenshot(&chapter, window_, "main-window",
                "Orbit connected to OrbitTest, before any capture has been taken.");
  manual_->AddChapter(std::move(chapter));
}

void Recorder::RecordSymbols() {
  Chapter chapter;
  chapter.id = "symbols";
  chapter.title = "Modules and functions";
  chapter.summary =
      "The Symbols tab lists the modules loaded into the target and the functions their symbols "
      "name.";
  chapter.paragraphs = {
      "Orbit cannot show a function it has no name for, so loading symbols is the first thing it "
      "does after connecting. The Symbols tab shows how far it got: every module the target has "
      "mapped, and whether its symbols were found.",
      "The lower list holds every function Orbit knows about. Selecting a function here is what "
      "marks it for dynamic instrumentation, so that the next capture times every call to it.",
  };

  ShowRightTab("SymbolsTab");

  QWidget* functions = GetWidget("FunctionsList");
  FitColumns(GetRightTabs());
  AddScreenshot(&chapter, GetRightTabs(), "symbols-tab",
                "The Symbols tab: modules on top, the functions found in them below.");

  SetFilter("FunctionsList", "FilterLineEdit", "TestFunc");
  FitColumns(functions);
  AddScreenshot(&chapter, functions, "symbols-filtered-functions",
                "Typing in the filter box narrows the function list; here, to OrbitTest's "
                "TestFunc and TestFunc2.");
  SetFilter("FunctionsList", "FilterLineEdit", "");

  manual_->AddChapter(std::move(chapter));
}

void Recorder::RecordCaptureOptions() {
  Chapter chapter;
  chapter.id = "capture-options";
  chapter.title = "Capture options";
  chapter.summary = "What Orbit collects while a capture runs, and how much of it.";
  chapter.paragraphs = {
      "Everything a capture records costs something to record. The capture options decide what is "
      "worth paying for on this run: the callstack sampling rate and unwinding method, whether "
      "thread states and scheduling are collected, whether memory usage is sampled, and whether "
      "the manual instrumentation compiled into the target is enabled.",
  };
  RecordModalDialog(&chapter, "actionCaptureOptions", "capture-options",
                    "The capture options dialog, reached from Settings.");
  manual_->AddChapter(std::move(chapter));
}

ErrorMessageOr<void> Recorder::RecordTakingACapture() {
  Chapter chapter;
  chapter.id = "taking-a-capture";
  chapter.title = "Taking a capture";
  chapter.summary =
      "One button starts and stops a capture; while it runs, the capture window fills in live.";
  chapter.paragraphs = {
      "The red button on the capture toolbar starts a capture and stops it again. The timer next "
      "to it counts the capture up, and the capture window draws events as they arrive rather "
      "than waiting for the end.",
      "Stopping a capture is when the analysis tabs on the right become available: they all "
      "summarise a capture that has finished arriving.",
  };

  QAction* toggle = GetAction("actionToggle_Capture");
  const bool can_capture =
      PumpEventsUntil([toggle]() { return toggle->isEnabled(); }, options_.ui_timeout);
  if (!can_capture) {
    return ErrorMessage{
        "The capture button never became enabled, which means Orbit does not consider the target "
        "process to be running. Without a capture there is no manual to write."};
  }

  toggle->trigger();
  // Let enough of the capture arrive that the screenshot is not of an empty window.
  PumpEventsFor(options_.capture_duration / 2);
  AddScreenshot(&chapter, window_, "capture-in-progress",
                "A capture in progress: tracks appear as events arrive, and the toolbar timer "
                "counts up.");

  PumpEventsFor(options_.capture_duration / 2);
  toggle->trigger();

  const bool capture_finished = PumpEventsUntil(
      [this]() { return IsRightTabEnabled("samplingTab"); }, options_.ui_timeout);
  if (!capture_finished) {
    return ErrorMessage{
        "The capture never finished being processed, so none of the analysis views have anything "
        "to show."};
  }
  PumpEventsFor(absl::Seconds(2));

  AddScreenshot(&chapter, window_, "capture-taken",
                "The same window once the capture has stopped and been processed.");
  manual_->AddChapter(std::move(chapter));
  return outcome::success();
}

void Recorder::RecordCaptureWindow() {
  Chapter chapter;
  chapter.id = "capture-window";
  chapter.title = "The capture window";
  chapter.summary = "A track per thread and per data source, on one shared timeline.";
  chapter.paragraphs = {
      "The capture window draws everything a capture recorded against time. Each thread gets a "
      "track; so does each other source of events, such as the scheduler or the GPU. Scrolling "
      "zooms the timeline, dragging pans it, and right-clicking a slice offers what can be done "
      "with it.",
      "OrbitTest runs a handful of worker threads that call the same few functions in a loop, so "
      "its tracks are regular in a way a real application's rarely are.",
  };

  SetCaptureWindowShare(0.75);
  AddScreenshot(&chapter, GetWidget("CaptureTab"), "capture-window",
                "The capture window, with one track per thread of the target.");

  SetFilter("FilterPanelWidget", "filterTracks", "OrbitTest");
  AddScreenshot(&chapter, GetWidget("CaptureTab"), "capture-window-filtered",
                "The track filter hides every track whose name does not match.");
  SetFilter("FilterPanelWidget", "filterTracks", "");

  manual_->AddChapter(std::move(chapter));
}

void Recorder::RecordTrackConfiguration() {
  Chapter chapter;
  chapter.id = "track-configuration";
  chapter.title = "Configuring tracks";
  chapter.summary = "Which kinds of track are shown, and in what order.";
  chapter.paragraphs = {
      "A capture of a real application has more tracks than fit on a screen. The track "
      "configuration pane, opened from the View menu, turns whole categories of track on and off "
      "and reorders what is left, so that what matters for the question at hand is next to each "
      "other.",
  };

  QAction* configure = GetAction("actionConfigureTracks");
  if (!configure->isEnabled()) {
    ORBIT_ERROR("Track configuration is disabled; skipping that chapter");
    return;
  }
  configure->setChecked(true);
  PumpEventsFor(absl::Milliseconds(750));

  AddScreenshot(&chapter, GetWidget("CaptureTab"), "track-configuration",
                "The track configuration pane, next to the tracks it controls.");

  configure->setChecked(false);
  SetCaptureWindowShare(0.4);
  manual_->AddChapter(std::move(chapter));
}

void Recorder::RecordLiveFunctions() {
  Chapter chapter;
  chapter.id = "live-functions";
  chapter.title = "Live functions";
  chapter.summary = "One row per instrumented function, with how often it ran and for how long.";
  chapter.paragraphs = {
      "The Live tab aggregates every timed function in the capture: call count, total time, "
      "average, minimum and maximum. It is the fastest way to find the function worth looking at "
      "before going back to the timeline to see when it ran.",
      "Selecting a row highlights every one of that function's slices in the capture window, and "
      "the histogram below the list shows how the individual call durations were distributed. The "
      "screenshot has the most-called function selected.",
  };

  ShowRightTab("liveTab");
  FitColumns(GetRightTabs());
  SelectFirstRow(GetWidget("liveFunctions"), "treeView");
  AddScreenshot(&chapter, GetRightTabs(), "live-functions",
                "The Live tab, with one row per function that the capture timed.");
  manual_->AddChapter(std::move(chapter));
}

void Recorder::RecordManualInstrumentation() {
  Chapter chapter;
  chapter.id = "manual-instrumentation";
  chapter.title = "Manual instrumentation";
  chapter.summary =
      "Scopes the target names itself, through the ORBIT_SCOPE macros, alongside everything else.";
  chapter.paragraphs = {
      "Dynamic instrumentation can time any function, but only a function. Code that wants to "
      "time part of one, or to name what it is doing, includes Orbit's API header and brackets "
      "the region with ORBIT_SCOPE. Those scopes arrive in a capture as ordinary timed slices, "
      "with the name and colour the target gave them.",
      "OrbitTest uses the API throughout, which is why names like \"Sleep for two milliseconds\" "
      "appear in a capture of it next to the function names that came from symbols.",
  };

  ShowRightTab("liveTab");
  SetFilter("liveFunctions", "FilterLineEdit", "Sleep");
  FitColumns(GetRightTabs());
  AddScreenshot(&chapter, GetRightTabs(), "manual-instrumentation",
                "Scopes that OrbitTest named through ORBIT_SCOPE, filtered out of the Live tab.");
  SetFilter("liveFunctions", "FilterLineEdit", "");
  manual_->AddChapter(std::move(chapter));
}

void Recorder::RecordSampling() {
  Chapter chapter;
  chapter.id = "sampling";
  chapter.title = "Sampling";
  chapter.summary =
      "Where the target spent its time, from periodically recorded callstacks rather than from "
      "instrumentation.";
  chapter.paragraphs = {
      "Instrumentation only sees functions that were asked for by name. Sampling sees everything: "
      "Orbit interrupts the target at a fixed rate and records where each thread was, so that a "
      "function nobody thought to instrument still shows up if it is where the time goes.",
      "The upper list ranks functions by how often a sample landed in them. Selecting one shows, "
      "below, the callstacks those samples came from.",
  };

  if (!IsRightTabEnabled("samplingTab")) {
    ORBIT_ERROR("The sampling tab is disabled; skipping that chapter");
    return;
  }
  ShowRightTab("samplingTab");
  PumpEventsFor(absl::Seconds(1));
  FitColumns(GetRightTabs());
  AddScreenshot(&chapter, GetRightTabs(), "sampling",
                "The Sampling tab, ranking functions by how many callstack samples landed in "
                "them.");
  manual_->AddChapter(std::move(chapter));
}

void Recorder::RecordCallTrees() {
  {
    Chapter chapter;
    chapter.id = "top-down";
    chapter.title = "Top-down";
    chapter.summary = "The sampled callstacks merged into a tree, from each thread's entry point.";
    chapter.paragraphs = {
        "The same samples the Sampling tab counts can be read as a tree instead of a list. The "
        "top-down view merges every callstack from the root down, so each node says how much of "
        "the capture was spent inside that call path. It answers \"what is this thread doing\" "
        "better than a flat list can.",
    };

    if (IsRightTabEnabled("topDownTab")) {
      ShowRightTab("topDownTab");
      if (auto* tree = GetWidget("topDownWidget")->findChild<QTreeView*>("callTreeTreeView");
          tree != nullptr) {
        tree->expandToDepth(2);
        PumpEventsFor(absl::Milliseconds(750));
      }
      AddScreenshot(&chapter, GetRightTabs(), "top-down",
                    "The top-down view, expanded a few levels into OrbitTest's worker threads.");
      manual_->AddChapter(std::move(chapter));
    } else {
      ORBIT_ERROR("The top-down tab is disabled; skipping that chapter");
    }
  }

  {
    Chapter chapter;
    chapter.id = "bottom-up";
    chapter.title = "Bottom-up";
    chapter.summary = "The same callstacks merged from the leaves, to find who calls a hot function.";
    chapter.paragraphs = {
        "The bottom-up view starts from the functions samples landed in and walks back towards "
        "the callers. Where the top-down view answers what a thread is doing, this one answers "
        "why a particular function is being called so much, and from where.",
    };

    if (IsRightTabEnabled("bottomUpTab")) {
      ShowRightTab("bottomUpTab");
      if (auto* tree = GetWidget("bottomUpWidget")->findChild<QTreeView*>("callTreeTreeView");
          tree != nullptr) {
        tree->expandToDepth(2);
        PumpEventsFor(absl::Milliseconds(750));
      }
      AddScreenshot(&chapter, GetRightTabs(), "bottom-up",
                    "The bottom-up view, expanded from the most sampled functions towards their "
                    "callers.");
      manual_->AddChapter(std::move(chapter));
    } else {
      ORBIT_ERROR("The bottom-up tab is disabled; skipping that chapter");
    }
  }
}

void Recorder::RecordCaptureLog() {
  Chapter chapter;
  chapter.id = "capture-log";
  chapter.title = "The capture log";
  chapter.summary = "What Orbit did while the capture ran, and anything that went wrong doing it.";
  chapter.paragraphs = {
      "Taking a capture involves a lot that can partly fail: symbols that were not found, "
      "functions that could not be instrumented, a tracepoint that was not permitted. The capture "
      "log, opened from the button in the status bar, records all of it with the time into the "
      "capture at which it happened.",
      "It is the first place to look when a capture is missing something it should have had.",
  };

  QPushButton* log_button = nullptr;
  for (QPushButton* button : window_->findChildren<QPushButton*>()) {
    if (button->accessibleName() == "CaptureLogButton") {
      log_button = button;
      break;
    }
  }
  if (log_button == nullptr || !log_button->isEnabled()) {
    ORBIT_ERROR("The capture log button was not found or is disabled; skipping that chapter");
    return;
  }

  log_button->setChecked(true);
  PumpEventsFor(absl::Milliseconds(750));
  AddScreenshot(&chapter, GetWidget("captureLogWidget"), "capture-log",
                "The capture log for the capture this manual was generated from.");
  log_button->setChecked(false);
  PumpEventsFor(absl::Milliseconds(250));
  manual_->AddChapter(std::move(chapter));
}

void Recorder::RecordSymbolLocations() {
  Chapter chapter;
  chapter.id = "symbol-locations";
  chapter.title = "Symbol locations";
  chapter.summary = "Where Orbit looks for the debug information it needs to name functions.";
  chapter.paragraphs = {
      "A stripped binary carries no function names, and Orbit is not much use without them. This "
      "dialog is where the directories holding separate debug files are listed, where a module "
      "can be pointed at a specific symbol file, and where the Microsoft symbol server can be "
      "turned on for Windows modules.",
  };
  RecordModalDialog(&chapter, "actionSymbolLocationsDialog", "symbol-locations",
                    "The symbol locations dialog, reached from Settings.");
  manual_->AddChapter(std::move(chapter));
}

void Recorder::RecordAbout() {
  Chapter chapter;
  chapter.id = "about";
  chapter.title = "About";
  chapter.summary = "Which build of Orbit produced this manual.";
  chapter.paragraphs = {
      "The About dialog carries the version and the build report. Quoting it is the fastest way "
      "to make a bug report reproducible.",
  };
  RecordModalDialog(&chapter, "actionAbout", "about",
                    "The About dialog of the build that generated this manual.");
  manual_->AddChapter(std::move(chapter));
}

ErrorMessageOr<void> Recorder::Record() {
  // Orbit pops message boxes of its own when something goes wrong -- a module whose symbols could
  // not be loaded, or a tracepoint that could not be opened. Left alone they would block the
  // script forever, so dismiss them and say so in the log.
  //
  // Only message boxes: Orbit also shows progress dialogs, for finalizing a capture among other
  // things, and closing one of those would cancel the work the manual is waiting for.
  auto* watchdog = new QTimer{window_};
  QObject::connect(watchdog, &QTimer::timeout, [this]() {
    if (expecting_modal_dialog_) return;
    auto* message_box = qobject_cast<QMessageBox*>(QApplication::activeModalWidget());
    if (message_box == nullptr) return;
    ORBIT_ERROR("Dismissing a message box that Orbit raised: \"%s\" -- %s",
                message_box->windowTitle().toStdString(), message_box->text().toStdString());
    message_box->close();
  });
  watchdog->start(1000);

  WaitForSymbols();
  RecordMainWindow();
  RecordSymbols();
  RecordCaptureOptions();
  OUTCOME_TRY(RecordTakingACapture());
  RecordCaptureWindow();
  RecordTrackConfiguration();
  RecordLiveFunctions();
  RecordManualInstrumentation();
  RecordSampling();
  RecordCallTrees();
  RecordCaptureLog();
  RecordSymbolLocations();
  RecordAbout();

  watchdog->stop();
  return outcome::success();
}

}  // namespace

ErrorMessageOr<void> RecordManual(OrbitMainWindow* window, Manual* manual,
                                  const RecordingOptions& options) {
  Recorder recorder{window, manual, options};
  return recorder.Record();
}

}  // namespace orbit_manual
