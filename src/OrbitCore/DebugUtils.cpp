#include "DebugUtils.h"

DebugManager& DebugManager::Get() {
  static DebugManager manager;
  return manager;
}

void DebugManager::RegisterStaticToggle(StaticToggle* toggle) {
  absl::MutexLock lock{&mutex_};
  static_toggles_.insert(toggle);
}