#ifndef _LIBUNWINDSTACK_COFF_H
#define _LIBUNWINDSTACK_COFF_H

#include <memory>
#include <mutex>
#include <vector>

#include <unwindstack/CoffInterface.h>
#include <unwindstack/Error.h>
#include <unwindstack/Memory.h>

namespace unwindstack {

// Forward declaration.
struct MapInfo;
class Regs;

struct DosHeader {
  uint16_t magic;
  // 29 * uint16_t fields
  uint32_t lfanew;  // Naming from lldb code.
};

struct CoffHeader {
  uint16_t nsects;
  uint16_t hdrsize;
};

struct CoffOptionalHeader {};

struct SectionHeader {
  char name[8];
  uint32_t vmsize;
  uint32_t vmaddr;
  uint32_t size;
  uint32_t offset;
  uint32_t reloff;
  uint32_t lineoff;
  uint16_t nrel;
  uint16_t nline;
  uint32_t flags;
};

class Coff {
 public:
  Coff(Memory* memory) : memory_(memory) {}
  virtual ~Coff() = default;

  bool Init();
  void Invalidate();

  /*
  bool StepIfSignalHandler(uint64_t rel_pc, Regs* regs, Memory* process_memory);
  bool Step(uint64_t rel_pc, Regs* regs, Memory* process_memory, bool* finished);
  */

  int64_t GetLoadBias() { return load_bias_; }

  bool IsValidPc(uint64_t pc);

  void GetLastError(ErrorData* data);
  ErrorCode GetLastErrorCode();
  uint64_t GetLastErrorAddress();

  bool valid() { return valid_; }
  Memory* memory() { return memory_.get(); }

 protected:
  bool ParseSectionHeaders(const CoffHeader& coff_header, Memory* memory, uint64_t* offset);
  bool ParseHeaders(Memory* memory);

  bool valid_ = false;
  int64_t load_bias_ = 0;
  std::unique_ptr<Memory> memory_;

  // Parsed data
  std::vector<SectionHeader> section_headers_;
  DosHeader dos_header_;
  CoffHeader coff_header_;
  CoffOptionalHeader coff_optional_header_;
};

}  // namespace unwindstack

#endif  // _LIBUNWINDSTACK_ELF_H
