
#include <unwindstack/Coff.h>

#include <capstone/capstone.h>
#include <capstone/x86.h>

#include <unwindstack/MachineX86_64.h>
#include <unwindstack/Regs.h>

#define LOG_TAG "unwind"
#include <log/log.h>

#include <array>

namespace unwindstack {

bool kVerboseLogging = false;

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
  uint64_t offset = 0x0;
  if (!Get16(memory, &offset, &(header->magic))) {
    return false;
  }
  uint32_t kImageDosSignature = 0x5A4d;
  ALOGI_IF(kVerboseLogging, "DOS signature read: %x", header->magic);
  ALOGI_IF(kVerboseLogging, "DOS signature expected: %x", kImageDosSignature);

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
  ALOGI_IF(kVerboseLogging, "ParseCoffOptionalHeader");
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
    ALOGI_IF(kVerboseLogging, "num_data_dir_entries: %u", header->num_data_dir_entries);
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
  ALOGI_IF(kVerboseLogging, "ParseSectionHeaders");

  uint32_t num_sections = coff_header.nsects;
  ALOGI_IF(kVerboseLogging, "Number of sections: %u", num_sections);

  for (uint32_t idx = 0; idx < num_sections; ++idx) {
    SectionHeader section_header;
    if (!memory->ReadFully(*offset, static_cast<void*>(&section_header.name[0]), 8)) {
      return false;
    }
    ALOGI_IF(kVerboseLogging, "section name: %s", section_header.name.data());
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
    ALOGI_IF(kVerboseLogging, "section rva: %x", section_header.vmaddr);
    ALOGI_IF(kVerboseLogging, "section offset: %x", section_header.offset);
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
  ALOGI_IF(kVerboseLogging, "PE signature read: %x", pe_signature);
  ALOGI_IF(kVerboseLogging, "PE signature expected: %x", kImagePeSignature);

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

void Coff::InitializeSections() {
  for (const auto& section_header : section_headers_) {
    Section section;
    std::string header_name(section_header.name.data(), section_header.name.size());
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
      ALOGI_IF(kVerboseLogging, ".pdata found");
      pdata_section_ = section;
    }
    if (section.name == ".xdata") {
      ALOGI_IF(kVerboseLogging, ".xdata found");
      xdata_section_ = section;
    }
  }
}

uint64_t MapFromRVAToFileOffset(const Section& section, uint64_t rva) {
  ALOGI_IF(kVerboseLogging, "section vmaddr: %x", section.vmaddr);
  ALOGI_IF(kVerboseLogging, "section offset: %x", section.offset);
  return rva - section.vmaddr + section.offset;
}

// The order of registers in PE/COFF unwind information is different from the libunwindstack
// register order, so we have to map them to the right values.
// https://docs.microsoft.com/en-us/cpp/build/exception-handling-x64?view=msvc-160#operation-info
static uint16_t MapToUnwindstackRegister(uint8_t op_info_register) {
  std::array<uint16_t, 16> kMachineToUnwindstackRegister = {
      X86_64_REG_RAX, X86_64_REG_RCX, X86_64_REG_RDX, X86_64_REG_RBX,
      X86_64_REG_RSP, X86_64_REG_RBP, X86_64_REG_RSI, X86_64_REG_RDI,
      X86_64_REG_R8,  X86_64_REG_R9,  X86_64_REG_R10, X86_64_REG_R11,
      X86_64_REG_R12, X86_64_REG_R13, X86_64_REG_R14, X86_64_REG_R15};

  if (op_info_register >= kMachineToUnwindstackRegister.size()) {
    return X86_64_REG_LAST;
  }

  return kMachineToUnwindstackRegister[op_info_register];
}

// Pre-condition: We know we are not in the epilog.
bool Coff::ProcessUnwindOpCodes(Memory* process_memory, Regs* regs, const UnwindInfo& unwind_info,
                                uint64_t current_code_offset) {
  ALOGI_IF(kVerboseLogging, "current offset from start: %lx", current_code_offset);
  int start_op_idx;
  // TODO: Need to handle correctly those unwind ops that are actually offsets.
  for (start_op_idx = 0; start_op_idx < unwind_info.num_codes; ++start_op_idx) {
    if (unwind_info.unwind_codes[start_op_idx].code_and_op.code_offset <= current_code_offset) {
      break;
    }
  }
  ALOGI_IF(kVerboseLogging, "current start index: %d", start_op_idx);

  RegsImpl<uint64_t>* cur_regs = reinterpret_cast<RegsImpl<uint64_t>*>(regs);

  ALOGI_IF(kVerboseLogging, "stack pointer start: %lx", cur_regs->sp());

  // // Process op codes.
  for (int op_idx = start_op_idx; op_idx < unwind_info.num_codes;) {
    UnwindCode unwind_code = unwind_info.unwind_codes[op_idx];
    switch (unwind_code.GetUnwindOp()) {
      case 0: {  // UWOP_PUSH_NONVOL; TODO: create enum/names.
        uint64_t register_value;
        if (!process_memory->Read64(cur_regs->sp(), &register_value)) {
          ALOGI_IF(kVerboseLogging, "Failed to read memory");
          return false;
        }
        cur_regs->set_sp(cur_regs->sp() + sizeof(uint64_t));
        ALOGI("stack pointer: %lx", cur_regs->sp());

        uint64_t value;
        process_memory->Read64(cur_regs->sp(), &value);
        ALOGI("value at sp: %lx", value);

        uint8_t op_info = unwind_code.GetOpInfo();
        uint16_t reg = MapToUnwindstackRegister(op_info);
        ALOGI("setting register: %x", reg);
        (*cur_regs)[reg] = register_value;

        op_idx++;
        break;
      }
      case 1: {  // UWOP_ALLOC_LARGE
        uint8_t op_info = unwind_code.GetOpInfo();
        uint32_t allocation_size = 0;

        if (op_info == 0) {
          if (op_idx + 1 > unwind_info.num_codes) {
            ALOGI_IF(kVerboseLogging, "Error parsing unwind info.");
            return false;
          }
          UnwindCode offset = unwind_info.unwind_codes[op_idx + 1];
          allocation_size = 8 * static_cast<uint32_t>(offset.frame_offset);
          op_idx += 1;
        } else if (op_info == 1) {
          if (op_idx + 2 > unwind_info.num_codes) {
            ALOGI_IF(kVerboseLogging, "Error parsing unwind info.");
            return false;
          }
          UnwindCode offset1 = unwind_info.unwind_codes[op_idx + 1];
          UnwindCode offset2 = unwind_info.unwind_codes[op_idx + 2];

          allocation_size = static_cast<uint32_t>(offset1.frame_offset) +
                            (static_cast<uint32_t>(offset2.frame_offset) << 16);
          op_idx += 2;
        }

        ALOGI("UWOP_ALLOC_LARGE allocation size: %x", allocation_size);

        cur_regs->set_sp(cur_regs->sp() + allocation_size);
        ALOGI("stack pointer: %lx", cur_regs->sp());

        uint64_t value;
        process_memory->Read64(cur_regs->sp(), &value);
        ALOGI("value at sp: %lx", value);

        op_idx += 1;

        break;
      }
      case 2: {  // UWOP_ALLOC_SMALL
        uint8_t op_info = unwind_code.GetOpInfo();
        uint32_t allocation_size = static_cast<uint32_t>(op_info) * 8 + 8;
        ALOGI_IF(kVerboseLogging, "UWOP_ALLOC_SMALL allocation size: %x", allocation_size);

        cur_regs->set_sp(cur_regs->sp() + allocation_size);
        ALOGI("stack pointer: %lx", cur_regs->sp());

        uint64_t value;
        process_memory->Read64(cur_regs->sp(), &value);
        ALOGI("value at sp: %lx", value);

        op_idx += 1;
        break;
      }
      default: {
        // TODO: Support all op codes.
        return false;
      }
    }
  }

  uint64_t return_address;
  if (!process_memory->Read64(cur_regs->sp(), &return_address)) {
    return false;
  }
  cur_regs->set_sp(cur_regs->sp() + sizeof(uint64_t));
  ALOGI("stack pointer: %lx", cur_regs->sp());
  cur_regs->set_pc(return_address);
  return true;
}

bool Coff::DetectAndHandleEpilog(uint64_t start_address, uint64_t end_address,
                                 uint64_t current_offset_from_start, Memory* process_memory,
                                 Regs* regs) {
  ALOGI_IF(kVerboseLogging, "Coff::DetectAndHandleEpilog");
  cs_insn* instruction = cs_malloc(capstone_handle_);
  size_t code_size = end_address - start_address - current_offset_from_start;
  std::vector<uint8_t> code_from_process;
  code_from_process.resize(code_size);
  // TODO: Use the map base address, not the one from the optional header?
  if (!process_memory->ReadFully(
          coff_optional_header_.image_base + start_address + current_offset_from_start,
          static_cast<void*>(&code_from_process[0]), code_size)) {
    ALOGI_IF(kVerboseLogging, "Reading from process memory failed");
    cs_free(instruction, 1);
    return false;
  }
  uint64_t current_offset = 0;
  const uint8_t* code_pointer = code_from_process.data();

  uint64_t rsp_adjustment = 0;

  bool is_first_iteration = true;

  while (code_size > 0) {
    ALOGI_IF(kVerboseLogging, "code size: %lx", code_size);
    ALOGI_IF(kVerboseLogging, "code vector size: %lx", code_from_process.size());
    ALOGI_IF(kVerboseLogging, "current_offset: %lx", current_offset);

    if (!cs_disasm_iter(capstone_handle_, &code_pointer, &code_size, &current_offset,
                        instruction)) {
      ALOGI_IF(kVerboseLogging, "Disassembling failed");
      cs_free(instruction, 1);
      return false;
    }

    // The instructions 'lea' and 'add' are only legal as the first instruction of the epilog,
    // so we can only see them in the first iteration of this loop if we are indeed in the
    // epilog (and in which case we are actually at the start of the epilog).
    if (is_first_iteration && instruction->id == X86_INS_LEA) {
      ALOGI_IF(kVerboseLogging, "lea instruction op string: %s", instruction->op_str);
      // TODO: Set rsp accordingly.
    } else if (is_first_iteration && instruction->id == X86_INS_ADD) {
      // TODO: Add proper value to rsp.
      ALOGI_IF(kVerboseLogging, "add instruction op string: %s", instruction->op_str);
    } else if (instruction->id == X86_INS_POP) {
      ALOGI_IF(kVerboseLogging, "pop instruction");
      // TODO: Set register accordingly.
      rsp_adjustment += 8;
    } else if (instruction->id == X86_INS_RET) {
      ALOGI_IF(kVerboseLogging, "return instruction");
      // This is the last instruction of the epilog.
      break;
    } else {
      cs_free(instruction, 1);
      return false;
    }

    is_first_iteration = false;

    ALOGI_IF(kVerboseLogging, "Instruction address: %lx", instruction->address);
    ALOGI_IF(kVerboseLogging, "Instruction mnemonic: %s", instruction->mnemonic);
    ALOGI_IF(kVerboseLogging, "Instruction op string: %s", instruction->op_str);
    ALOGI_IF(kVerboseLogging, "Instruction name: %s",
             cs_insn_name(capstone_handle_, instruction->id));

    current_offset += instruction->size;
  }

  RegsImpl<uint64_t>* cur_regs = reinterpret_cast<RegsImpl<uint64_t>*>(regs);
  cur_regs->set_sp(cur_regs->sp() + rsp_adjustment);
  ALOGI("stack pointer (epilog handling): %lx", cur_regs->sp());

  cs_free(instruction, 1);

  return true;
}

// Experimental code for trying out stuff.
bool Coff::ParseExceptionTableExperimental(Memory* object_file_memory, Memory* process_memory,
                                           Regs* regs, uint64_t pc_rva) {
  constexpr int kCoffDataDirExceptionTableIndex = 3;
  if (kCoffDataDirExceptionTableIndex >= coff_optional_header_.data_dirs.size()) {
    ALOGI_IF(kVerboseLogging, "No exception table found.");
    return false;
  }
  DataDirectory data_directory = coff_optional_header_.data_dirs[kCoffDataDirExceptionTableIndex];
  if (data_directory.vmaddr == 0) {
    ALOGI_IF(kVerboseLogging, "No exception table found.");
    return false;
  }
  constexpr uint16_t kImageFileMachineAmd64 = 0x8664;
  if (coff_header_.machine != kImageFileMachineAmd64) {
    ALOGI_IF(kVerboseLogging, "Unsupported machine type.");
    return false;
  }

  ALOGI_IF(kVerboseLogging, "PC relative virtual address: %lx", pc_rva);

  uint32_t rva = data_directory.vmaddr;
  uint32_t size = data_directory.vmsize;
  ALOGI_IF(kVerboseLogging, "Exception table rva: %x", rva);
  ALOGI_IF(kVerboseLogging, "Exception table size: %x", size);

  uint64_t pdata_file_offset = MapFromRVAToFileOffset(pdata_section_, rva);
  ALOGI_IF(kVerboseLogging, "Exception table file offset: %lx", pdata_file_offset);
  ALOGI_IF(kVerboseLogging, "Exception table size: %x", size);

  uint64_t end = pdata_file_offset + size;
  // TODO: Can do binary search, but we just do linear search for simplicity for now.
  RuntimeFunction function_at_pc;
  bool runtime_function_found = false;
  for (uint64_t offset = pdata_file_offset; offset < end;) {
    RuntimeFunction function;
    if (!Get32(object_file_memory, &offset, &(function.start_address)) ||
        !Get32(object_file_memory, &offset, &(function.end_address)) ||
        !Get32(object_file_memory, &offset, &(function.unwind_info_offset))) {
      ALOGI_IF(kVerboseLogging, "ERROR: Unexpected read error.");
      break;
    }

    // TODO: Is end address inclusive?
    if (pc_rva >= function.start_address && pc_rva <= function.end_address) {
      function_at_pc = function;
      runtime_function_found = true;
    }
  }

  if (!runtime_function_found) {
    ALOGI_IF(kVerboseLogging, "No RUNTIME_FUNCTION found.");
    RegsImpl<uint64_t>* cur_regs = reinterpret_cast<RegsImpl<uint64_t>*>(regs);

    uint64_t return_address;
    if (!process_memory->Read64(cur_regs->sp(), &return_address)) {
      return false;
    }
    cur_regs->set_pc(return_address);
    cur_regs->set_sp(cur_regs->sp() + sizeof(uint64_t));
    return true;
  }

  ALOGI_IF(kVerboseLogging, "function found start address: %x", function_at_pc.start_address);
  ALOGI_IF(kVerboseLogging, "function found end address: %x", function_at_pc.end_address);
  ALOGI_IF(kVerboseLogging, "function found unwind info offset: %x",
           function_at_pc.unwind_info_offset);

  uint64_t xdata_file_offset =
      MapFromRVAToFileOffset(xdata_section_, function_at_pc.unwind_info_offset);
  ALOGI_IF(kVerboseLogging, "xdata info file offset: %lx", xdata_file_offset);

  UnwindInfo unwind_info;
  if (!Get8(object_file_memory, &xdata_file_offset, &(unwind_info.version_and_flags)) ||
      !Get8(object_file_memory, &xdata_file_offset, &(unwind_info.prolog_size)) ||
      !Get8(object_file_memory, &xdata_file_offset, &(unwind_info.num_codes)) ||
      !Get8(object_file_memory, &xdata_file_offset, &(unwind_info.frame_register_and_offset))) {
    return false;
  }

  uint64_t current_offset_from_start = pc_rva - function_at_pc.start_address;

  if (  // current_offset_from_start > function_at_pc.start_address + unwind_info.prolog_size &&
      DetectAndHandleEpilog(function_at_pc.start_address, function_at_pc.end_address,
                            current_offset_from_start, process_memory, regs)) {
    return true;
  }

  ALOGI_IF(kVerboseLogging, "count of unwind codes: %u", unwind_info.num_codes);
  ALOGI_IF(kVerboseLogging, "unwind code version: %u", unwind_info.GetVersion());

  // TODO: Handle versions?
  assert(unwind_info.GetVersion() == 0x01);

  // TODO: Handle flags.
  assert(unwind_info.GetFlags() == 0x00);

  for (uint8_t code_idx = 0; code_idx < unwind_info.num_codes; ++code_idx) {
    UnwindCode unwind_code;
    if (!Get8(object_file_memory, &xdata_file_offset, &(unwind_code.code_and_op.code_offset)) ||
        !Get8(object_file_memory, &xdata_file_offset,
              &(unwind_code.code_and_op.unwind_op_and_op_info))) {
      return false;
    }
    ALOGI_IF(kVerboseLogging, "unwind code_offset: %x", unwind_code.code_and_op.code_offset);

    ALOGI("unwind code: %u", unwind_code.GetUnwindOp());
    ALOGI("unwind op info: %u", unwind_code.GetOpInfo());
    unwind_info.unwind_codes.emplace_back(unwind_code);
  }

  if (!ProcessUnwindOpCodes(process_memory, regs, unwind_info, current_offset_from_start)) {
    ALOGI_IF(kVerboseLogging, "Failed to process unwind op codes.");
    return false;
  }

  return true;
}

bool Coff::Step(uint64_t rel_pc, Regs* regs, Memory* process_memory, bool* finished) {
  // Lock during the step which can update information in the object.
  std::lock_guard<std::mutex> guard(lock_);

  ALOGI("PC before step: %lx", regs->pc());
  ALOGI("SP before step: %lx", regs->sp());

  ALOGI_IF(kVerboseLogging, "Coff::Step() call");
  ALOGI_IF(kVerboseLogging, "Rel pc: %lx", rel_pc);
  ALOGI_IF(kVerboseLogging, "Image base: %lx", coff_optional_header_.image_base);

  uint64_t pc_rva = rel_pc - coff_optional_header_.image_base;
  if (!ParseExceptionTableExperimental(memory_.get(), process_memory, regs, pc_rva)) {
    ALOGI_IF(kVerboseLogging, "Coff unwinding step failed.");
    *finished = true;
    return false;
  }

  ALOGI("PC after step: %lx", regs->pc());
  ALOGI("SP after step: %lx", regs->sp());
  *finished = (regs->pc() == 0) ? true : false;

  assert(false);

  return true;
}

bool Coff::InitCapstone() {
  // TODO: Support 32-bit mode.
  cs_err err = cs_open(CS_ARCH_X86, CS_MODE_64, &capstone_handle_);
  if (err) {
    ALOGI_IF(kVerboseLogging, "Failed to initialize Capstone library.");
    return false;
  }
  return true;
}

bool Coff::Init() {
  std::lock_guard<std::mutex> guard(lock_);

  ALOGI_IF(kVerboseLogging, "Coff::Init()");
  ParseHeaders(memory_.get());
  InitializeSections();
  InitCapstone();
  return true;
}

}  // namespace unwindstack