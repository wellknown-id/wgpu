export class Inspector {
  constructor() {}
  init() {}
  setRenderer(_renderer) {}
  begin() {}
  finish() {}
  inspect() {}
  computeAsync() {}
  beginCompute() {}
  finishCompute() {}
  beginRender() {}
  finishRender() {}
  createParameters(_name) {
    return {
      add() { return { name() { return this; } }; },
    };
  }
}
