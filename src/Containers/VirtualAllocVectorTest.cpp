// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include <string>
#include <utility>
#include <deque>

#include "Containers/BlockChain.h"
#include "Containers/VirtualAllocVector.h"

namespace orbit_containers {

namespace {

class CopyableType {
 public:
  CopyableType() = default;
  explicit CopyableType(std::string value) : value_{std::move(value)} {}

  CopyableType(const CopyableType&) = default;
  CopyableType& operator=(const CopyableType&) = default;

  void set_value(std::string value) { value_ = std::move(value); }
  [[nodiscard]] const std::string& value() const { return value_; }

 private:
  std::string value_;
};

class MovableType {
 public:
  MovableType() = default;
  explicit MovableType(std::string value) : value_{std::move(value)} {}

  MovableType(const MovableType&) = delete;
  MovableType& operator=(const MovableType&) = delete;

  MovableType(MovableType&&) = default;
  MovableType& operator=(MovableType&&) = default;

  [[nodiscard]] const std::string& value() const { return value_; }

 private:
  std::string value_;
};

};  // namespace

TEST(VirtualAllocVector, AddCopyableTypes) {
  CopyableType v1("hello world");
  CopyableType v2("or not");

  VirtualAllocVector<CopyableType> chain(1024 * 1024);
  EXPECT_EQ(chain.size(), 0);
  chain.emplace_back(v1);
  chain.emplace_back(v2);
  EXPECT_EQ(chain.size(), 2);

  v1.set_value("new v1");
  v2.set_value("new v2");

  EXPECT_EQ(chain[0].value(), "hello world");
  EXPECT_EQ(chain[1].value(), "or not");

  // Multi-block test
  for (int i = 0; i < 2000; ++i) {
    chain.emplace_back(v1);
  }
  EXPECT_EQ(chain.size(), 2002);
}

TEST(VirtualAllocVector, Clear) {
  const std::string v1 = "hello world";
  const std::string v2 = "or not";

  VirtualAllocVector<CopyableType> chain(1024 * 1024);
  chain.emplace_back(v1);
  EXPECT_GT(chain.size(), 0);
  chain.clear();
  EXPECT_EQ(chain.size(), 0);

  chain.emplace_back(v2);
  EXPECT_GT(chain.size(), 0);

  // Multi-block test
  for (int i = 0; i < 2000; ++i) {
    chain.emplace_back(v1);
  }
  chain.clear();
  EXPECT_EQ(chain.size(), 0);
}

TEST(VirtualAllocVector, ElementIteration) {
  //   constexpr const int v1 = 5;
  //   constexpr const int v2 = 10;
  //   constexpr const int v3 = 15;

  //   BlockChain<int, 1024> chain;

  //   chain.emplace_back(v1);
  //   chain.emplace_back(v2);
  //   chain.emplace_back(v3);

  //   // Note that only the "++it" operator is supported
  //   auto it = chain.begin();
  //   EXPECT_EQ(*it, v1);
  //   ++it;
  //   EXPECT_EQ(*it, v2);
  //   ++it;
  //   EXPECT_EQ(*it, v3);
  //   ++it;
  //   // ... and also only !=, not ==
  //   EXPECT_FALSE(it != chain.end());

  //   // Test the complete "typical pattern"
  //   int it_count = 0;
  //   for (it = chain.begin(); it != chain.end(); ++it) {
  //     ++it_count;
  //   }
  //   EXPECT_EQ(it_count, 3);

  //   // Multi-block test
  //   chain.clear();
  //   for (int i = 0; i < 2000; ++i) {
  //     chain.emplace_back(i);
  //   }
  //   it_count = 0;

  //   for (it = chain.begin(); it != chain.end(); ++it) {
  //     EXPECT_EQ(*it, it_count);
  //     ++it_count;
  //   }

  //   auto it_begin = chain.begin();
  //   it = chain.begin();
  //   ++it;
  //   while (it != chain.end()) {
  //     EXPECT_TRUE(it != it_begin);
  //     ++it;
  //   }

  //   EXPECT_EQ(it_count, 2000);
}

TEST(VirtualAllocVector, EmptyIteration) {
  //   BlockChain<std::string, 1024> chain;
  //   auto it = chain.begin();
  //   ASSERT_FALSE(it != chain.end());
}

TEST(VirtualAllocVector, AddCopyableTypesN) {
  const std::string v1 = "hello world";
  VirtualAllocVector<std::string> chain(1024 * 1024);
  chain.push_back_n(v1, 2000);
  EXPECT_EQ(chain.size(), 2000);
  /*for (auto& it : chain) {
    EXPECT_EQ(it, v1);
  }*/
}

// "Reset" works like "clear", except that it does not actually free
// any memory - it keeps the block's memory, just setting their
// size to 0
TEST(VirtualAllocVector, Reset) {
  //   VirtualAllocVector<CopyableType> chain(1024 * 1024);
  //   chain.push_back_n(5, 1024 * 3);
  //   EXPECT_GT(chain.size(), 0);
  //   const Block<int, 1024>* blockPtr[] = {chain.root(), nullptr, nullptr};
  //   blockPtr[1] = blockPtr[0]->next();
  //   blockPtr[2] = blockPtr[1]->next();

  //   // Tests below rely quite a lot on the internals of BlockChain, but this
  //   // seems the easiest way to actually test re-usage of the block pointers
  //   chain.Reset();
  //   EXPECT_EQ(chain.size(), 0);
  //   EXPECT_EQ(blockPtr[0]->size(), 0);
  //   EXPECT_EQ(blockPtr[1]->size(), 0);
  //   EXPECT_EQ(blockPtr[2]->size(), 0);

  //   chain.push_back_n(10, 1024);
  //   EXPECT_GT(chain.size(), 0);
  //   EXPECT_EQ(chain.root()->data()[0], 10);
  //   EXPECT_EQ(chain.root(), blockPtr[0]);
  //   EXPECT_EQ(chain.root()->next(), blockPtr[1]);
  //   EXPECT_EQ(blockPtr[1]->size(), 0);

  //   chain.push_back_n(10, 1024);
  //   EXPECT_EQ(chain.root()->next(), blockPtr[1]);
  //   EXPECT_EQ(blockPtr[1]->size(), 1024);
  //   EXPECT_EQ(blockPtr[2]->size(), 0);

  //   chain.push_back_n(10, 1024);
  //   EXPECT_EQ(chain.root()->next()->next(), blockPtr[2]);
  //   EXPECT_EQ(blockPtr[2]->size(), 1024);
}

TEST(VirtualAllocVector, MovableType) {
  
  VirtualAllocVector<MovableType> chain(1024 * 1024);
  EXPECT_EQ(chain.size(), 0);
  chain.emplace_back(MovableType("v1"));
  chain.emplace_back(MovableType("v2"));
  EXPECT_EQ(chain.size(), 2);

  /*EXPECT_EQ(chain.root()->data()[0].value(), "v1");
  EXPECT_EQ(chain.root()->data()[1].value(), "v2");*/
}

struct S {
  uint64_t m_0;
  uint64_t m_1;
  uint64_t m_2;
  uint64_t m_3;
  uint64_t m_4;
  uint64_t m_5;
  uint64_t m_6;
  uint64_t m_7;
  uint64_t m_8;
  uint64_t m_9;
};

template<typename VectorT>
void InsertElements(VectorT& vector, size_t size, S s, std::string_view name) {
  size_t vec_size = 0;
  {
    ORBIT_SCOPED_TIMED_LOG(name);
    for (size_t i = 0; i < size; ++i) {
      vector.emplace_back(s);
    }
    vec_size = vector.size();
  }
  //ORBIT_LOG("%s size = %u",name, vec_size);
}

TEST(VirtualAllocVector, Performance) {
  constexpr size_t kNumElements = 100 * 1024 * 1024;
  static S s;
  s.m_0 = 777;
  s.m_1 = 0xFFFFFFFFFFFFFFFF;
  s.m_2 = 0xfefefefefefefefe;
  s.m_3 = 0xC0C0C0C0CC0C0C0C;
  s.m_4 = 0xABCDEFABCDEF0000;
  s.m_5 = 0xFFFFFFFFFFFFFFFF;
  s.m_6 = 0xfefefefefefefefe;
  s.m_7 = 0xC0C0C0C0CC0C0C0C;
  s.m_8 = 0xABCDEFABCDEF0000;
  s.m_9 = 0xABCDEFABCDEF0000;

  std::vector<S> reserved_std_vector;
  reserved_std_vector.reserve(kNumElements);
  InsertElements(reserved_std_vector, kNumElements, s, "Reserved std::vector");

  std::vector<S> std_vector;
  InsertElements(std_vector, kNumElements, s, "std::vector");

  /*std::deque<S> std_deque;
    InsertElements(std_deque, kNumElements, s, "std::deque");*/

  for (size_t i = 0; i < 10; ++i) {
    VirtualAllocVector<S> virtual_alloc_vector;
    InsertElements(virtual_alloc_vector, kNumElements, s, "VirtualAllocVector");

    BlockChain<S, 64 * 1024> block_chain;
    InsertElements(block_chain, kNumElements, s, "BlockChain");
  }
}

}  // namespace orbit_containers