//-----------------------------------
// Copyright Pierric Gimmig 2013-2017
//-----------------------------------
#pragma once

#include <vector>

#include "DataView.h"
#include "OrbitType.h"

class TypesDataView : public DataView {
 public:
  TypesDataView();

  const std::vector<Column>& GetColumns() override;
  int GetDefaultSortingColumn() override { return COLUMN_NAME; }
  std::vector<std::string> GetContextMenu(
      int a_ClickedIndex, const std::vector<int>& a_SelectedIndices) override;
  std::string GetValue(int a_Row, int a_Column) override;

  void OnFilter(const std::string& a_Filter) override;
  void ParallelFilter(const std::string& a_Filter);
  void OnSort(int a_Column, std::optional<SortingOrder> a_NewOrder) override;
  void OnContextMenu(const std::string& a_Action, int a_MenuIndex,
                     const std::vector<int>& a_ItemIndices) override;
  void OnDataChanged() override;

 protected:
  Type& GetType(unsigned int a_Row) const;

  void OnProp(const std::vector<int>& a_Items);
  void OnView(const std::vector<int>& a_Items);
  void OnClip(const std::vector<int>& a_Items);

  std::vector<std::string> m_FilterTokens;

  enum ColumnIndex {
    COLUMN_INDEX,
    COLUMN_NAME,
    COLUMN_LENGTH,
    COLUMN_TYPE_ID,
    COLUMN_TYPE_ID_UNMOD,
    COLUMN_NUM_VARIABLES,
    COLUMN_NUM_FUNCTIONS,
    COLUMN_NUM_BASE_CLASSES,
    COLUMN_BASE_OFFSET,
    COLUMN_MODULE,
    COLUMN_NUM
  };

  static const std::string MENU_ACTION_SUMMARY;
  static const std::string MENU_ACTION_DETAILS;
};
