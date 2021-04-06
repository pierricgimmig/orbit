#include "Introspection/DebugUtils.h"

#include "OrbitBase/Logging.h"

DebugManager& DebugManager::Get() {
  static DebugManager manager;
  return manager;
}

void DebugManager::RegisterStaticToggle(StaticToggle* toggle) {
  absl::MutexLock lock{&mutex_};
  const std::string& name = toggle->GetFullName();
  CHECK(static_toggles_by_full_name.count(name) == 0);
  static_toggles_by_full_name[name] = toggle;
}

std::vector<StaticToggle*> DebugManager::GetSortedStaticToggles() {
  absl::MutexLock lock{&mutex_};
  std::vector<StaticToggle*> toggles;
  for (auto& [unused_name, toggle] : static_toggles_by_full_name) {
    toggles.push_back(toggle);
  }
  return toggles;
}
