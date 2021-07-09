#ifndef _LIBUNWINDSTACK_COFF_H
#define _LIBUNWINDSTACK_COFF_H

#include <memory>
#include <mutex>
#include <vector>

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
  uint16_t machine;
  uint16_t nsects;
  uint16_t hdrsize;
};

struct DataDirectory {
  uint32_t vmaddr;
  uint32_t vmsize;
};

struct CoffOptionalHeader {
  uint16_t magic = 0;
  uint8_t major_linker_version = 0;
  uint8_t minor_linker_version = 0;
  uint32_t code_size = 0;
  uint32_t data_size = 0;
  uint32_t bss_size = 0;
  uint32_t entry = 0;
  uint32_t code_offset = 0;
  uint32_t data_offset = 0;
  uint64_t image_base = 0;
  uint32_t sect_alignment = 0;
  uint32_t file_alignment = 0;
  uint16_t major_os_system_version = 0;
  uint16_t minor_os_system_version = 0;
  uint16_t major_image_version = 0;
  uint16_t minor_image_version = 0;
  uint16_t major_subsystem_version = 0;
  uint16_t minor_subsystem_version = 0;
  uint32_t reserved1 = 0;
  uint32_t image_size = 0;
  uint32_t header_size = 0;
  uint32_t checksum = 0;
  uint16_t subsystem = 0;
  uint16_t dll_flags = 0;
  uint64_t stack_reserve_size = 0;
  uint64_t stack_commit_size = 0;
  uint64_t heap_reserve_size = 0;
  uint64_t heap_commit_size = 0;
  uint32_t loader_flags = 0;
  uint32_t	num_data_dir_entries;
  std::vector<DataDirectory> data_dirs;  // will contain num_data_dir_entries entries
};

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

struct Section {
  std::string name;
  uint32_t vmsize;
  uint32_t vmaddr;
  uint32_t size;  // Size in file
  uint32_t offset;  // Offset in file
};

class Coff {
 public:
  Coff(Memory* memory) : memory_(memory) {}
  virtual ~Coff() = default;

  bool Init();
  void Invalidate();

  /*
  bool StepIfSignalHandler(uint64_t rel_pc, Regs* regs, Memory* process_memory);
  */
  bool Step(uint64_t rel_pc, Regs* regs, Memory* process_memory, bool* finished);

  int64_t GetLoadBias() { return load_bias_; }

  bool IsValidPc(uint64_t pc);

  void GetLastError(ErrorData* data);
  ErrorCode GetLastErrorCode();
  uint64_t GetLastErrorAddress();

  bool valid() { return valid_; }
  Memory* memory() { return memory_.get(); }

 protected:
  bool ParseSectionHeaders(const CoffHeader& coff_header, Memory* memory, uint64_t* offset);
  void InitializeSections();
  bool ParseHeaders(Memory* memory);
  bool ParseExceptionTableExperimental(Memory* memory, uint64_t pc_rva);

  bool valid_ = false;
  int64_t load_bias_ = 0;
  std::unique_ptr<Memory> memory_;

  std::vector<Section> sections_;
  Section pdata_section_;
  Section xdata_section_;

  // Parsed data
  std::vector<SectionHeader> section_headers_;
  DosHeader dos_header_;
  CoffHeader coff_header_;
  CoffOptionalHeader coff_optional_header_;
};

}  // namespace unwindstack

#endif  // _LIBUNWINDSTACK_ELF_H
