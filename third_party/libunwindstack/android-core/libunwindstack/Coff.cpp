
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

struct Function {
  uint32_t start_address;
  uint32_t end_address;
  uint32_t unwind_info_offset;
};

// Experimental code for trying out stuff.
bool Coff::ParseExceptionTableExperimental(Memory* memory) {
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

  return true;
}

bool Coff::Step(uint64_t rel_pc, Regs* regs, Memory* process_memory, bool* finished) {
  ALOGI("Coff::Step() call");
  ALOGI("Rel pc: %lx", rel_pc);
  ALOGI("Image base: %lx", coff_optional_header_.image_base);
  return true;
}

bool Coff::Init() {
  ALOGI("Coff::Init()");
  ParseHeaders(memory_.get());
  ParseExceptionTableExperimental(memory_.get());
  return true;
}

}  // namespace unwindstack