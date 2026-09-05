// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "OrbitManual/Screenshots.h"

#include <absl/strings/str_cat.h>
#include <absl/time/clock.h>
#include <absl/time/time.h>

#include <QApplication>
#include <QElapsedTimer>
#include <QEventLoop>
#include <QImage>
#include <QOpenGLWidget>
#include <QPainter>
#include <QPixmap>
#include <QPoint>
#include <QRect>
#include <QString>
#include <QWidget>
#include <functional>
#include <string>
#include <string_view>

#include "OrbitBase/Logging.h"
#include "OrbitBase/Result.h"

namespace orbit_manual {

namespace {

// Long enough that the event loop makes progress, short enough that a predicate that has just
// become true is noticed right away.
constexpr int kPumpSliceMs = 20;

}  // namespace

void PumpEventsFor(absl::Duration duration) {
  QElapsedTimer timer;
  timer.start();
  const qint64 duration_ms = absl::ToInt64Milliseconds(duration);
  while (timer.elapsed() < duration_ms) {
    QApplication::processEvents(QEventLoop::AllEvents, kPumpSliceMs);
  }
}

bool PumpEventsUntil(const std::function<bool()>& predicate, absl::Duration timeout) {
  QElapsedTimer timer;
  timer.start();
  const qint64 timeout_ms = absl::ToInt64Milliseconds(timeout);
  while (timer.elapsed() < timeout_ms) {
    if (predicate()) return true;
    QApplication::processEvents(QEventLoop::AllEvents, kPumpSliceMs);
  }
  return predicate();
}

ErrorMessageOr<std::string> Screenshotter::Grab(QWidget* widget, std::string_view name) {
  ORBIT_CHECK(widget != nullptr);

  if (!widget->isVisible()) {
    return ErrorMessage{absl::StrCat("Cannot grab \"", name, "\": it is not visible")};
  }

  // Let anything that is still queued to repaint do so before the widget is asked to paint itself.
  QApplication::processEvents();

  QPixmap pixmap = widget->grab();
  if (pixmap.isNull()) {
    return ErrorMessage{absl::StrCat("Grabbing \"", name, "\" produced no image")};
  }

  // QWidget::grab() asks each widget to paint itself, which a QOpenGLWidget cannot do: it renders
  // into a framebuffer object that only the compositing path knows about, and leaves a blank
  // rectangle behind here. Orbit's capture window is one of those, so read the framebuffers and
  // paint them in.
  QPainter painter{&pixmap};
  for (QOpenGLWidget* gl_widget : widget->findChildren<QOpenGLWidget*>()) {
    if (!gl_widget->isVisible()) continue;
    const QImage framebuffer = gl_widget->grabFramebuffer();
    if (framebuffer.isNull()) {
      ORBIT_ERROR("Framebuffer of \"%s\" could not be read for \"%s\"",
                  gl_widget->objectName().toStdString(), std::string{name});
      continue;
    }
    painter.drawImage(QRect{gl_widget->mapTo(widget, QPoint{0, 0}), gl_widget->size()},
                      framebuffer);
  }
  painter.end();

  const std::string file_name = absl::StrCat(name, ".png");
  const std::filesystem::path path = image_directory_ / file_name;
  if (!pixmap.save(QString::fromStdString(path.string()), "PNG")) {
    return ErrorMessage{absl::StrCat("Unable to write ", path.string())};
  }

  ORBIT_LOG("Wrote %s (%dx%d)", path.string(), pixmap.width(), pixmap.height());
  return file_name;
}

}  // namespace orbit_manual
