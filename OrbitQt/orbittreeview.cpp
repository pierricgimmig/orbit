// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This needs to be first because if it is not GL/glew.h
// complains about being included after gl.h
// clang-format off
#include "OpenGl.h"
// clang-format on

#include "orbittreeview.h"

#include <QApplication>
#include <QFontDatabase>
#include <QHeaderView>
#include <QKeyEvent>
#include <QMenu>
#include <QPainter>
#include <QScrollBar>
#include <QSignalMapper>
#include <set>
#include <utility>

#include "App.h"
#include "DataView.h"
#include "orbitglwidget.h"

OrbitTreeView::OrbitTreeView(QWidget* parent) : QTreeView(parent), auto_resize_(true) {
  header()->setSortIndicatorShown(true);
  header()->setSectionsClickable(true);

  setRootIsDecorated(false);
  setItemsExpandable(false);
  setContextMenuPolicy(Qt::CustomContextMenu);
  setSelectionBehavior(QTreeView::SelectRows);
  setTextElideMode(Qt::ElideMiddle);

  connect(header(), SIGNAL(sortIndicatorChanged(int, Qt::SortOrder)), this,
          SLOT(OnSort(int, Qt::SortOrder)));

  connect(this, SIGNAL(customContextMenuRequested(const QPoint&)), this,
          SLOT(ShowContextMenu(const QPoint&)));

  connect(header(), SIGNAL(sectionResized(int, int, int)), this,
          SLOT(columnResized(int, int, int)));

  connect(verticalScrollBar(), SIGNAL(rangeChanged(int, int)), this,
          SLOT(OnRangeChanged(int, int)));
}

void OrbitTreeView::Initialize(DataView* data_view, SelectionType selection_type,
                               FontType font_type, bool uniform_row_height,
                               QFlags<Qt::AlignmentFlag> text_alignment) {
  setUniformRowHeights(uniform_row_height);

  model_ = std::make_unique<OrbitTableModel>(data_view, /*parent=*/nullptr, text_alignment);
  setModel(model_.get());
  header()->resizeSections(QHeaderView::ResizeToContents);

  if (!model_->IsSortingAllowed()) {
    // Don't do setSortingEnabled(model_->IsSortingAllowed()); as with true it
    // forces a sort by the first column.
    setSortingEnabled(false);
  } else {
    std::pair<int, Qt::SortOrder> column_and_order = model_->GetDefaultSortingColumnAndOrder();
    sortByColumn(column_and_order.first, column_and_order.second);
  }

  if (model_->GetUpdatePeriodMs() > 0) {
    timer_ = std::make_unique<QTimer>();
    connect(timer_.get(), SIGNAL(timeout()), this, SLOT(OnTimer()));
    timer_->start(model_->GetUpdatePeriodMs());
  }

  if (selection_type == SelectionType::kExtended) {
    setSelectionMode(ExtendedSelection);
  }

  setAlternatingRowColors(true);

  if (font_type == FontType::kFixed) {
    const QFont fixed_font = QFontDatabase::systemFont(QFontDatabase::FixedFont);
    setFont(fixed_font);
  }
}

void OrbitTreeView::SetDataModel(DataView* data_view) {
  model_ = std::make_unique<OrbitTableModel>();
  model_->SetDataView(data_view);
  setModel(model_.get());
}

void OrbitTreeView::OnSort(int section, Qt::SortOrder order) {
  model_->sort(section, order);
  Refresh();
}

void OrbitTreeView::OnFilter(const QString& filter) {
  model_->OnFilter(filter);
  Refresh();
}

void OrbitTreeView::OnTimer() {
  if (isVisible() && !model_->GetDataView()->SkipTimer()) {
    model_->OnTimer();
    Refresh();
  }
}

void OrbitTreeView::Refresh() {
  QModelIndexList list = selectionModel()->selectedIndexes();

  if (model_->GetDataView()->GetType() == DataViewType::kLiveFunctions) {
    model_->layoutAboutToBeChanged();
    model_->layoutChanged();
    return;
  }

  reset();

  // Re-select previous selection
  int selected = model_->GetSelectedIndex();
  if (selected >= 0) {
    QItemSelectionModel* selection = selectionModel();
    QModelIndex idx = model_->CreateIndex(selected, 0);

    // Don't re-trigger row selection callback when re-selecting.
    is_internal_refresh_ = true;
    selection->select(idx, QItemSelectionModel::ClearAndSelect | QItemSelectionModel::Rows);
    is_internal_refresh_ = false;
  }
}

void OrbitTreeView::resizeEvent(QResizeEvent* event) {
  if (auto_resize_ && model_ && model_->GetDataView()) {
    QSize header_size = size();
    for (size_t i = 0; i < model_->GetDataView()->GetColumns().size(); ++i) {
      float ratio = model_->GetDataView()->GetColumns()[i].ratio;
      if (ratio > 0.f) {
        header()->resizeSection(i, static_cast<int>(header_size.width() * ratio));
      }
    }
  }

  QTreeView::resizeEvent(event);
}

void OrbitTreeView::Link(OrbitTreeView* link) {
  links_.push_back(link);

  DataView* data_view = link->GetModel()->GetDataView();
  model_->GetDataView()->LinkDataView(data_view);
}

void OrbitTreeView::SetGlWidget(OrbitGLWidget* gl_widget) {
  model_->GetDataView()->SetGlCanvas(gl_widget->GetCanvas());
}

void OrbitTreeView::drawRow(QPainter* painter, const QStyleOptionViewItem& options,
                            const QModelIndex& index) const {
  QTreeView::drawRow(painter, options, index);
}

QMenu* GContextMenu;

void OrbitTreeView::ShowContextMenu(const QPoint& pos) {
  QModelIndex index = indexAt(pos);
  if (index.isValid()) {
    int clicked_index = index.row();

    QModelIndexList selection_list = selectionModel()->selectedIndexes();
    std::set<int> selection_set;
    for (QModelIndex& selected_index : selection_list) {
      selection_set.insert(selected_index.row());
    }
    std::vector<int> selected_indices(selection_set.begin(), selection_set.end());

    std::vector<std::string> menu =
        model_->GetDataView()->GetContextMenu(clicked_index, selected_indices);
    if (!menu.empty()) {
      QMenu context_menu(tr("ContextMenu"), this);
      GContextMenu = &context_menu;
      std::vector<std::unique_ptr<QAction>> actions;

      for (size_t i = 0; i < menu.size(); ++i) {
        actions.push_back(std::make_unique<QAction>(QString::fromStdString(menu[i])));
        connect(actions[i].get(), &QAction::triggered,
                [this, &menu, i] { OnMenuClicked(menu[i], i); });
        context_menu.addAction(actions[i].get());
      }

      context_menu.exec(mapToGlobal(pos));
      GContextMenu = nullptr;
    }
  }
}

void OrbitTreeView::OnMenuClicked(const std::string& action, int menu_index) {
  QModelIndexList selection_list = selectionModel()->selectedIndexes();
  std::set<int> selection_set;
  for (QModelIndex& index : selection_list) {
    selection_set.insert(index.row());
  }

  std::vector<int> indices(selection_set.begin(), selection_set.end());
  if (!indices.empty()) {
    model_->GetDataView()->OnContextMenu(action, menu_index, indices);
  }
}

void OrbitTreeView::keyPressEvent(QKeyEvent* event) {
  if (event->matches(QKeySequence::Copy)) {
    QModelIndexList list = selectionModel()->selectedIndexes();
    std::set<int> selection;
    for (QModelIndex& index : list) {
      selection.insert(index.row());
    }

    std::vector<int> items(selection.begin(), selection.end());
    model_->GetDataView()->CopySelection(items);
  } else {
    QTreeView::keyPressEvent(event);
  }
}

void OrbitTreeView::selectionChanged(const QItemSelection& selected,
                                     const QItemSelection& deselected) {
  QTreeView::selectionChanged(selected, deselected);

  // Don't trigger callbacks if selection was initiated internally.
  if (is_internal_refresh_) return;

  // Row selection callback.
  QModelIndex index = selectionModel()->currentIndex();
  if (index.isValid()) {
    OnRowSelected(index.row());
  }
}

void OrbitTreeView::OnRowSelected(int row) {
  model_->OnRowSelected(row);
  for (OrbitTreeView* tree_view : links_) {
    tree_view->Refresh();
  }
}

void OrbitTreeView::OnRangeChanged(int /*min*/, int max) {
  DataView* data_view = model_->GetDataView();
  if (data_view->ScrollToBottom()) {
    verticalScrollBar()->setValue(max);
  }
}

std::string OrbitTreeView::GetLabel() {
  if (model_ != nullptr && model_->GetDataView() != nullptr) {
    return model_->GetDataView()->GetLabel();
  }
  return "";
}

bool OrbitTreeView::HasRefreshButton() const {
  if (model_ != nullptr && model_->GetDataView() != nullptr) {
    return model_->GetDataView()->HasRefreshButton();
  }
  return false;
}

void OrbitTreeView::OnRefreshButtonClicked() {
  if (model_ != nullptr && model_->GetDataView() != nullptr) {
    model_->GetDataView()->OnRefreshButtonClicked();
  }
}

void OrbitTreeView::columnResized(int /*column*/, int /*oldSize*/, int /*newSize*/) {
  if (QApplication::mouseButtons() == Qt::LeftButton) {
    auto_resize_ = false;
  }
}
