
#include <unwindstack/Coff.h>

#define LOG_TAG "unwind"
#include <log/log.h>

#include <array>

namespace unwindstack {

namespace {

bool Get8(Memory* memory, uint64_t* offset, uint8_t* value) {
  if (memory->Read8(*offset, value)) {
    *offset += sizeof(uint8_t);
    return true;
  }
  return false;
}

bool Get16(Memory* memory, uint64_t* offset, uint16_t* value) {
  if (memory->Read16(*offset, value)) {
    *offset += sizeof(uint16_t);
    return true;
  }
  return false;
}

bool Get32(Memory* memory, uint64_t* offset, uint32_t* value) {
  if (memory->Read32(*offset, value)) {
    *offset += sizeof(uint32_t);
    return true;
  }
  return false;
}

bool Get64(Memory* memory, uint64_t* offset, uint64_t* value) {
  if (memory->Read64(*offset, value)) {
    *offset += sizeof(uint64_t);
    return true;
  }
  return false;
}

bool GetMax64(Memory* memory, uint64_t* offset, uint64_t size, uint64_t* value) {
  switch (size) {
    case 1:
      uint8_t value8;
      if (!Get8(memory, offset, &value8)) {
        return false;
      }
      *value = value8;
      return true;
    case 2:
      uint16_t value16;
      if (!Get16(memory, offset, &value16)) {
        return false;
      }
      *value = value16;
      return true;
    case 4:
      uint32_t value32;
      if (!Get32(memory, offset, &value32)) {
        return false;
      }
      *value = value32;
      return true;
    case 8:
      return Get64(memory, offset, value);
    default:
      assert(false);
      return false;
  }
  assert(false);
  return false;
}

bool ParseDosHeader(Memory* memory, DosHeader* header) {
  ALOGI("ParseDosHeader");
  uint64_t offset = 0x0;
  if (!Get16(memory, &offset, &(header->magic))) {
    return false;
  }
  uint32_t kImageDosSignature = 0x5A4d;
  ALOGI("DOS signature read: %x", header->magic);
  ALOGI("DOS signature expected: %x", kImageDosSignature);

  offset += 29 * sizeof(uint16_t);
  if (!Get32(memory, &offset, &(header->lfanew))) {
    return false;
  }
  return true;
}

bool ParseCoffHeader(Memory* memory, uint64_t* offset, CoffHeader* header) {
  if (!Get16(memory, offset, &(header->machine)) || !Get16(memory, offset, &(header->nsects))) {
    return false;
  }
  *offset += 3 * sizeof(uint32_t);
  if (!Get16(memory, offset, &(header->hdrsize))) {
    return false;
  }
  *offset += sizeof(uint16_t);
  return true;
}

bool ParseCoffOptionalHeader(const CoffHeader& coff_header, Memory* memory, uint64_t* offset,
                             CoffOptionalHeader* header) {
  ALOGI("ParseCoffOptionalHeader");
  uint64_t end_offset = *offset + coff_header.hdrsize;
  if (*offset < end_offset) {
    if (!Get16(memory, offset, &(header->magic)) ||
        !Get8(memory, offset, &(header->major_linker_version)) ||
        !Get8(memory, offset, &(header->minor_linker_version)) ||
        !Get32(memory, offset, &(header->code_size)) ||
        !Get32(memory, offset, &(header->data_size)) ||
        !Get32(memory, offset, &(header->bss_size)) || !Get32(memory, offset, &(header->entry)) ||
        !Get32(memory, offset, &(header->code_offset))) {
      return false;
    }
  }
  if (*offset < end_offset) {
    constexpr uint32_t kOptionalHeaderMagicPE32 = 0x010b;
    if (header->magic == kOptionalHeaderMagicPE32) {
      assert(false);
      if (!Get32(memory, offset, &(header->data_offset))) {
        return false;
      }
    } else {
      header->data_offset = 0;
    }
  }
  // TODO: Do not assume this is a 64-bit binary.
  uint64_t addr_byte_size = 8;
  constexpr uint32_t kOptionalHeaderMagicPE32Plus = 0x020b;
  assert(header->magic == kOptionalHeaderMagicPE32Plus);

  if (*offset < end_offset) {
    if (!GetMax64(memory, offset, addr_byte_size, &(header->image_base)) ||
        !Get32(memory, offset, &(header->sect_alignment)) ||
        !Get32(memory, offset, &(header->file_alignment)) ||
        !Get16(memory, offset, &(header->major_os_system_version)) ||
        !Get16(memory, offset, &(header->minor_os_system_version)) ||
        !Get16(memory, offset, &(header->major_image_version)) ||
        !Get16(memory, offset, &(header->minor_image_version)) ||
        !Get16(memory, offset, &(header->major_subsystem_version)) ||
        !Get16(memory, offset, &(header->minor_subsystem_version)) ||
        !Get32(memory, offset, &(header->reserved1)) ||
        !Get32(memory, offset, &(header->image_size)) ||
        !Get32(memory, offset, &(header->header_size)) ||
        !Get32(memory, offset, &(header->checksum)) ||
        !Get16(memory, offset, &(header->subsystem)) ||
        !Get16(memory, offset, &(header->dll_flags)) ||
        !GetMax64(memory, offset, addr_byte_size, &(header->stack_reserve_size)) ||
        !GetMax64(memory, offset, addr_byte_size, &(header->stack_commit_size)) ||
        !GetMax64(memory, offset, addr_byte_size, &(header->heap_reserve_size)) ||
        !GetMax64(memory, offset, addr_byte_size, &(header->heap_commit_size)) ||
        !Get32(memory, offset, &(header->loader_flags)) ||
        !Get32(memory, offset, &(header->num_data_dir_entries))) {
      return false;
    }
    header->data_dirs.clear();
    header->data_dirs.resize(header->num_data_dir_entries);
    ALOGI("num_data_dir_entries: %u", header->num_data_dir_entries);
    for (uint32_t i = 0; i < header->num_data_dir_entries; ++i) {
      if (!Get32(memory, offset, &(header->data_dirs[i].vmaddr)) ||
          !Get32(memory, offset, &(header->data_dirs[i].vmsize))) {
        return false;
      }
    }
  }
  *offset = end_offset;
  return true;
}

}  // namespace

bool Coff::ParseSectionHeaders(const CoffHeader& coff_header, Memory* memory, uint64_t* offset) {
  ALOGI("ParseSectionHeaders");

  uint32_t num_sections = coff_header.nsects;
  ALOGI("Number of sections: %u", num_sections);

  for (uint32_t idx = 0; idx < num_sections; ++idx) {
    SectionHeader section_header;
    if (!memory->ReadFully(*offset, static_cast<void*>(&section_header.name[0]), 8)) {
      return false;
    }
    ALOGI("section name: %s", section_header.name);
    *offset += 8 * sizeof(char);
    if (!Get32(memory, offset, &section_header.vmsize) ||
        !Get32(memory, offset, &section_header.vmaddr) ||
        !Get32(memory, offset, &section_header.size) ||
        !Get32(memory, offset, &section_header.offset) ||
        !Get32(memory, offset, &section_header.reloff) ||
        !Get32(memory, offset, &section_header.lineoff) ||
        !Get16(memory, offset, &section_header.nrel) ||
        !Get16(memory, offset, &section_header.nline) ||
        !Get32(memory, offset, &section_header.flags)) {
      return false;
    }
    ALOGI("section rva: %x", section_header.vmaddr);
    ALOGI("section offset: %x", section_header.offset);
    section_headers_.emplace_back(section_header);
  }
  return true;
}

bool Coff::ParseHeaders(Memory* memory) {
  if (!ParseDosHeader(memory, &dos_header_)) {
    return false;
  }
  uint64_t offset = dos_header_.lfanew;
  uint32_t pe_signature;
  if (!Get32(memory, &offset, &pe_signature)) {
    return false;
  }
  uint32_t kImagePeSignature = 0x00004550;
  ALOGI("PE signature read: %x", pe_signature);
  ALOGI("PE signature expected: %x", kImagePeSignature);

  if (!ParseCoffHeader(memory, &offset, &coff_header_)) {
    return false;
  }
  if (coff_header_.hdrsize > 0) {
    if (!ParseCoffOptionalHeader(coff_header_, memory, &offset, &coff_optional_header_)) {
      return false;
    }
  }
  if (!ParseSectionHeaders(coff_header_, memory, &offset)) {
    return false;
  }

  return true;
}

template <class T, std::size_t N>
constexpr inline size_t LengthOfArray(T (&)[N]) {
  return N;
}

void Coff::InitializeSections() {
  for (const auto& section_header : section_headers_) {
    Section section;
    std::string header_name(section_header.name, LengthOfArray(section_header.name));
    std::string name_trimmed = header_name.substr(0, header_name.find('\0'));
    // TODO: Names that start with "/" need to be looked up at an offset.
    section.name = name_trimmed;
    section.vmaddr = section_header.vmaddr;
    section.vmsize = section_header.vmsize;
    section.size = section_header.size;
    section.offset = section_header.offset;
    sections_.emplace_back(section);
  }

  for (const auto& section : sections_) {
    if (section.name == ".pdata") {
      ALOGI(".pdata found");
      pdata_section_ = section;
    }
    if (section.name == ".xdata") {
      ALOGI(".xdata found");
      xdata_section_ = section;
    }
  }
}

// Data from RUNTIMEFUNCTION array.
struct RuntimeFunction {
  uint32_t start_address;
  uint32_t end_address;
  uint32_t unwind_info_offset;
};

uint64_t MapFromRVAToFileOffset(const Section& section, uint64_t rva) {
  ALOGI("section vmaddr: %x", section.vmaddr);
  ALOGI("section offset: %x", section.offset);
  return rva - section.vmaddr + section.offset;
}

struct UnwindCode {
  uint8_t code_offset;
  uint8_t unwind_op_and_op_info;

  uint8_t GetUnwindOp() const { return unwind_op_and_op_info & 0x0f; }
  uint8_t GetOpInfo() const { return (unwind_op_and_op_info >> 4) & 0x0f; }
};

// UNWIND_INFO struct
struct UnwindInfo {
  // First 3 bits are the version, other 5 bits are the flags.
  uint8_t version_and_flags;
  uint8_t prolog_size;
  uint8_t num_codes;
  // First 4 bits frame register, second 4 bits frame register offset
  uint8_t frame_register_and_offset;
  std::vector<UnwindCode> unwind_codes;

  uint8_t GetVersion() const { return version_and_flags & 0x07; }

  uint8_t GetFlags() const { return (version_and_flags >> 3) & 0x1f; }

  uint8_t GetFrameRegister() const { return frame_register_and_offset & 0x0f; }

  uint8_t GetFrameOffset() const { return (frame_register_and_offset >> 4) & 0x0f; }
};

// Experimental code for trying out stuff.
bool Coff::ParseExceptionTableExperimental(Memory* memory, uint64_t pc_rva) {
  constexpr int kCoffDataDirExceptionTableIndex = 3;
  if (kCoffDataDirExceptionTableIndex >= coff_optional_header_.data_dirs.size()) {
    ALOGI("No exception table found.");
    return false;
  }
  DataDirectory data_directory = coff_optional_header_.data_dirs[kCoffDataDirExceptionTableIndex];
  if (data_directory.vmaddr == 0) {
    ALOGI("No exception table found.");
    return false;
  }
  constexpr uint16_t kImageFileMachineAmd64 = 0x8664;
  if (coff_header_.machine != kImageFileMachineAmd64) {
    ALOGI("Unsupported machine type.");
    return false;
  }

  uint32_t rva = data_directory.vmaddr;
  uint32_t size = data_directory.vmsize;
  ALOGI("Exception table rva: %x", rva);
  ALOGI("Exception table size: %x", size);

  uint64_t pdata_file_offset = MapFromRVAToFileOffset(pdata_section_, rva);
  ALOGI("Exception table file offset: %lx", pdata_file_offset);
  ALOGI("Exception table size: %x", size);

  uint64_t end = pdata_file_offset + size;
  // TODO: Can do binary search, but we just do linear search for simplicity for now.
  RuntimeFunction function_at_pc;
  for (uint64_t offset = pdata_file_offset; offset < end;) {
    RuntimeFunction function;
    if (!Get32(memory, &offset, &(function.start_address)) ||
        !Get32(memory, &offset, &(function.end_address)) ||
        !Get32(memory, &offset, &(function.unwind_info_offset))) {
      ALOGI("ERROR: Unexpected read error.");
      break;
    }

    if (pc_rva >= function.start_address && pc_rva <= function.end_address) {
      function_at_pc = function;
    }
  }

  ALOGI("function start address: %x", function_at_pc.start_address);
  ALOGI("function end address: %x", function_at_pc.end_address);
  ALOGI("function unwind info offset: %x", function_at_pc.unwind_info_offset);

  uint64_t xdata_file_offset =
      MapFromRVAToFileOffset(xdata_section_, function_at_pc.unwind_info_offset);
  ALOGI("xdata info file offset: %lx", xdata_file_offset);

  UnwindInfo unwind_info;
  if (!Get8(memory, &xdata_file_offset, &(unwind_info.version_and_flags)) ||
      !Get8(memory, &xdata_file_offset, &(unwind_info.prolog_size)) ||
      !Get8(memory, &xdata_file_offset, &(unwind_info.num_codes)) ||
      !Get8(memory, &xdata_file_offset, &(unwind_info.frame_register_and_offset))) {
    return false;
  }

  ALOGI("count of unwind codes: %u", unwind_info.num_codes);
  ALOGI("unwind code version: %u", unwind_info.GetVersion());

  // TODO: Handle versions?
  assert(unwind_info.GetVersion() == 0x01);

  // TODO: Handle flags.
  assert(unwind_info.GetFlags() == 0x00);

  for (uint8_t code_idx = 0; code_idx < unwind_info.num_codes; ++code_idx) {
    UnwindCode unwind_code;
    if (!Get8(memory, &xdata_file_offset, &(unwind_code.code_offset)) ||
        !Get8(memory, &xdata_file_offset, &(unwind_code.unwind_op_and_op_info))) {
      return false;
    }
    ALOGI("unwind code_offset: %x", unwind_code.code_offset);
    ALOGI("unwind op info: %u", unwind_code.GetOpInfo());
    ALOGI("unwind code: %u", unwind_code.GetUnwindOp());
    unwind_info.unwind_codes.emplace_back(unwind_code);
  }

  return true;
}

bool Coff::Step(uint64_t rel_pc, Regs* regs, Memory* memory, bool* finished) {
  ALOGI("Coff::Step() call");
  ALOGI("Rel pc: %lx", rel_pc);
  ALOGI("Image base: %lx", coff_optional_header_.image_base);
  uint64_t pc_rva = rel_pc - coff_optional_header_.image_base;
  ParseExceptionTableExperimental(memory, pc_rva);

  return true;
}

bool Coff::Init() {
  ALOGI("Coff::Init()");
  ParseHeaders(memory_.get());
  InitializeSections();
  return true;
}

}  // namespace unwindstack