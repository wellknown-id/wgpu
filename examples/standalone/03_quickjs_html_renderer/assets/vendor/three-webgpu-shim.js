import * as THREE from './three.module.js';

export * from './three.module.js';

export class WebGPURenderer {
  constructor(parameters = {}) {
    this.parameters = parameters;
    this.domElement = {
      style: {},
      width: 1,
      height: 1,
    };
    this._pixelRatio = 1;
    this._width = 1;
    this._height = 1;
    this._uploadedGeometry = new WeakSet();
  }

  setPixelRatio(pixelRatio) {
    this._pixelRatio = pixelRatio || 1;
  }

  setSize(width, height) {
    this._width = width;
    this._height = height;
    this.domElement.width = Math.max(1, Math.floor(width * this._pixelRatio));
    this.domElement.height = Math.max(1, Math.floor(height * this._pixelRatio));
  }

  setAnimationLoop(callback) {
    __hostSetAnimationLoop(callback);
  }

  render(scene, camera) {
    const mesh = scene.children.find((child) => child && child.isInstancedMesh);
    if (!mesh) {
      return;
    }

    scene.updateMatrixWorld(true);
    camera.updateMatrixWorld(true);

    if (!this._uploadedGeometry.has(mesh.geometry)) {
      const position = mesh.geometry.getAttribute('position');
      const normal = mesh.geometry.getAttribute('normal');
      const index = mesh.geometry.index;

      __hostSetGeometry({
        positions: Array.from(position.array),
        normals: normal ? Array.from(normal.array) : [],
        indices: index ? Array.from(index.array) : [],
      });

      this._uploadedGeometry.add(mesh.geometry);
    }

    __hostRenderFrame({
      projectionMatrix: Array.from(camera.projectionMatrix.elements),
      viewMatrix: Array.from(camera.matrixWorldInverse.elements),
      worldMatrix: Array.from(mesh.matrixWorld.elements),
      instanceMatrices: Array.from(mesh.instanceMatrix.array.slice(0, mesh.count * 16)),
      count: mesh.count,
      clearColor: [0.03, 0.04, 0.06, 1.0],
    });
  }
}

export default THREE;
