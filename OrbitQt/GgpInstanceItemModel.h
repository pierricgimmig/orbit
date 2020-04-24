#ifndef ORBITQT_GGP_INSTANCE_ITEM_MODEL_H_
#define ORBITQT_GGP_INSTANCE_ITEM_MODEL_H_

#include <QAbstractItemModel>
#include <QModelIndex>
#include <QObject>
#include <QVariant>
#include <QVector>

#include "GgpInstance.h"
#include "OrbitBase/Logging.h"

class GgpInstanceItemModel : public QAbstractItemModel {
 public:
  explicit GgpInstanceItemModel(QVector<GgpInstance> instances = {},
                                QObject* parent = nullptr);

  void SetInstances(QVector<GgpInstance> instances);

  int columnCount(const QModelIndex& parent = {}) const override;
  QVariant data(const QModelIndex& index,
                int role = Qt::DisplayRole) const override;
  QVariant headerData(int section, Qt::Orientation orientation,
                      int role = Qt::DisplayRole) const override;
  QModelIndex index(int row, int col,
                    const QModelIndex& parent = {}) const override;
  QModelIndex parent(const QModelIndex& parent) const override;
  int rowCount(const QModelIndex& parent = {}) const override;

 private:
  QVector<GgpInstance> instances_;
};

#endif  // ORBITQT_GGP_INSTANCE_ITEM_MODEL_H_