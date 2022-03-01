// Copyright 2011-2022, Molecular Matters GmbH <office@molecular-matters.com>
// See LICENSE.txt for licensing details (2-clause BSD License:
// https://opensource.org/licenses/BSD-2-Clause)

#include "PDB.h"

#include <cmath>
#include <vector>

#include "PDB_PCH.h"
#include "PDB_RawFile.h"
#include "PDB_Types.h"
#include "PDB_Util.h"

#define PRINT_U(x) printf("%s = %u\n", #x, x)

class BlockPrinter {
 public:
  BlockPrinter(const void* data, const PDB::SuperBlock* super_block)
      : data_(data), super_block_(super_block) {}
  BlockPrinter() = delete;

  void PrintBlockAsUint32(uint32_t block_id) PDB_NO_EXCEPT {
    const uint32_t* block_data = reinterpret_cast<const uint32_t*>(GetBlock(block_id));
    printf("\nBlock #%u [0x%p]:\n", block_id, block_data);
    
    size_t num_elements = super_block_->blockSize / sizeof(uint32_t);
    for (int i = 0; i < num_elements; ++i) {
      printf("%u ", block_data[i]);
      if ((i + 1) % 16 == 0) {
        printf("\n");
      }
    }
  }

  PDB_NO_DISCARD const void* GetBlock(uint32_t block_id) PDB_NO_EXCEPT {
    const size_t file_offset =
        PDB::ConvertBlockIndexToFileOffset(block_id, super_block_->blockSize);
    const void* block_data = PDB::Pointer::Offset<const void*>(data_, file_offset);
    return block_data;
  }

 private:
  const void* data_ = nullptr;
  const PDB::SuperBlock* super_block_ = nullptr;
};

// ------------------------------------------------------------------------------------------------
// ------------------------------------------------------------------------------------------------
PDB_NO_DISCARD PDB::ErrorCode PDB::ValidateFile(const void* data) PDB_NO_EXCEPT {
  // validate the super block
  const SuperBlock* superBlock = Pointer::Offset<const SuperBlock*>(data, 0u);
  const uint32_t directoryBlockCount =
      ConvertSizeToBlockCount(superBlock->directorySize, superBlock->blockSize);
  {
    // validate header magic
    if (std::memcmp(superBlock->fileMagic, SuperBlock::MAGIC, sizeof(SuperBlock::MAGIC) != 0)) {
      return ErrorCode::InvalidSuperBlock;
    }

    // validate directory
    {
      // the directory is a block which consists of a list of block indices (uint32_t).
      // we cannot deal with directories being larger than a single block.
      const uint32_t blockIndicesPerBlock = superBlock->blockSize / sizeof(uint32_t);
      if (directoryBlockCount > blockIndicesPerBlock) {
        uint32_t num_indices_blocks = std::ceil(static_cast<float>(directoryBlockCount) /
                                                static_cast<float>(blockIndicesPerBlock));

        PRINT_U(directoryBlockCount);
        PRINT_U(blockIndicesPerBlock);
        PRINT_U(superBlock->blockSize);
        PRINT_U(superBlock->freeBlockMapIndex);
        PRINT_U(superBlock->blockCount);
        PRINT_U(superBlock->directorySize);
        PRINT_U(superBlock->unknown);
        PRINT_U(superBlock->directoryIndicesBlockIndex);
        PRINT_U(superBlock->directorySize / superBlock->blockSize);
        PRINT_U(num_indices_blocks);

        BlockPrinter block_printer(data, superBlock);
        std::vector<const void*> indice_block_addresses;
        for (int i = 0; i < num_indices_blocks; ++i) {
          uint32_t block_id = superBlock->directoryIndicesBlockIndex + i;
          block_printer.PrintBlockAsUint32(block_id);
          indice_block_addresses.push_back(block_printer.GetBlock(block_id));
        }

        // Check if indice blocks are contiguous in memory.
        const uint8_t* last_address = reinterpret_cast<const uint8_t*>(indice_block_addresses[0]);
        bool are_indice_blocks_contiguous = true;
        printf("\nContiguous indice blocks validation:\n");
        for (size_t i = 1; i < indice_block_addresses.size(); ++i) {
          const uint8_t* current_address =
              reinterpret_cast<const uint8_t*>(indice_block_addresses[i]);
          uint32_t offset_from_previous_block = current_address - last_address;
          if (offset_from_previous_block != superBlock->blockSize) {
            are_indice_blocks_contiguous = false;
          }

          PRINT_U(offset_from_previous_block);
          last_address = current_address;
        }

        printf("Indice blocks are contiguous: %s\n\n", are_indice_blocks_contiguous ? "true" : "false");

        // We assume that if a single block can't hold all the block indices, then we are spilling
        // the indices into subsequent blocks. Also, if those blocks are contiguous in memory, then
        // we can treat the blocks as a single buffer and index safely starting from the first block
        // address. Note that if the blocks weren't contiguous, we could allocate a new buffer to
        // hold the block indices.
        if (!are_indice_blocks_contiguous) {
          return ErrorCode::UnhandledDirectorySize;
        }
      }
    }

    // validate free block map.
    // the free block map should always reside at either index 1 or 2.
    if (superBlock->freeBlockMapIndex != 1u && superBlock->freeBlockMapIndex != 2u) {
      return ErrorCode::InvalidFreeBlockMap;
    }
  }

  return ErrorCode::Success;
}

// ------------------------------------------------------------------------------------------------
// ------------------------------------------------------------------------------------------------
PDB_NO_DISCARD PDB::RawFile PDB::CreateRawFile(const void* data) PDB_NO_EXCEPT {
  return RawFile(data);
}
