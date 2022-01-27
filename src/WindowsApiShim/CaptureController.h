// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_WINDOWS_API_SHIM_SHIM_CAPTURE_CONTROLLER_H_
#define ORBIT_WINDOWS_API_SHIM_SHIM_CAPTURE_CONTROLLER_H_

#include "CaptureEventProducer/CaptureEventProducer.h"
#include "ProducerSideChannel/ProducerSideChannel.h"

#include <vector>

namespace orbit_windows_api_shim {

// The `ShimCaptureController` controls the activation and deactivation of the shim based on events
// received from the service. It uses the OnCaptureStart/OnCaptureStop/OnCaptureFinished interface
// of the `CaptureEventProducer` but does not actually generate events. Manual instrumentation is
// used instead to send events to the service.
class CaptureController : public orbit_capture_event_producer::CaptureEventProducer {
 public:
  CaptureController() {
    BuildAndStart(orbit_producer_side_channel::CreateProducerSideChannel());
  }

  ~CaptureController() { ShutdownAndWait(); }

 protected:
  // Subclasses need to override this method to be notified of a request to start a capture.
  // This is also the chance for the subclasses to read or store the CaptureOptions.
  void OnCaptureStart(orbit_grpc_protos::CaptureOptions capture_options) override;
  // Subclasses need to override this method to be notified of a request to stop the capture.
  void OnCaptureStop() override;
  // Subclasses need to override this method to be notified that the current capture has finished.
  void OnCaptureFinished() override;

  private:
  std::vector<void*> target_functions_;
   orbit_grpc_protos::CaptureOptions capture_options_;
};

}  // namespace orbit_windows_api_shim

#endif  // ORBIT_WINDOWS_API_SHIM_SHIM_CAPTURE_CONTROLLER_H_