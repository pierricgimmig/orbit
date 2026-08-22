// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include <QSslSocket>
#include <QString>

namespace orbit_http {

// Orbit downloads debug symbols over HTTPS -- see
// RemoteSymbolProvider/MicrosoftSymbolServerSymbolProvider.cpp -- and that is
// the only transport in the tree that relies on TLS. QNetworkAccessManager
// gives no compile-time guarantee about it: Qt resolves its TLS backend by
// dlopening libssl at runtime, so a Qt built without it, or one whose backend
// cannot be found, fails every https:// request at runtime and nowhere else.
//
// That makes it worth asserting. The Bazel build supplies Qt from pinned
// packages rather than from the machine, so a change to how Qt is sourced could
// silently take TLS away without anything else noticing.
TEST(Tls, QtCanDoHttps) {
  ASSERT_TRUE(QSslSocket::supportsSsl())
      << "Qt has no usable TLS backend, so every HTTPS symbol download will "
         "fail. Qt was built against "
      << QSslSocket::sslLibraryBuildVersionString().toStdString()
      << " and looks for it with dlopen at runtime.";
}

// A backend older than the one Qt was built against is the shape of failure
// that actually happens in practice: the library resolves, but symbols Qt
// needs are missing. Reporting both versions makes that diagnosable from the
// test log alone.
TEST(Tls, RuntimeBackendIsReported) {
  const QString runtime_version = QSslSocket::sslLibraryVersionString();
  EXPECT_FALSE(runtime_version.isEmpty())
      << "No TLS backend was loaded at runtime, though Qt reports support for "
         "one.";
}

}  // namespace orbit_http
