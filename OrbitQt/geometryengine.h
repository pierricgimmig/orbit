#ifndef GEOMETRYENGINE_H
#define GEOMETRYENGINE_H

#include <QOpenGLBuffer>
#include <QOpenGLFunctions>
#include <QOpenGLShaderProgram>

class GeometryEngine : protected QOpenGLFunctions {
 public:
  GeometryEngine();
  virtual ~GeometryEngine();

  void drawCubeGeometry(QOpenGLShaderProgram* program);

 private:
  void initCubeGeometry();

  QOpenGLBuffer arrayBuf;
  QOpenGLBuffer indexBuf;
};

#endif  // GEOMETRYENGINE_H