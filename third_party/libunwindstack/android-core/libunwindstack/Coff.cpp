
#include <unwindstack/Coff.h>

#define LOG_TAG "unwind"
#include <log/log.h>

#include <array>

namespace unwindstack {

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
  *offset += sizeof(uint16_t);
  if (!Get16(memory, offset, &(header->nsects))) {
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
  *offset += coff_header.hdrsize;
  return true;
}

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

bool Coff::Init() {
  ALOGI("Coff::Init()");
  ParseHeaders(memory_.get());
  return true;
}

}  // namespace unwindstack