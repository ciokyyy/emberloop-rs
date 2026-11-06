// jsdom has no layout: every element measures 0×0 and there is no ResizeObserver, so a virtualizer
// would compute an empty window and render nothing. These stubs give the feed a real viewport and a
// scroll height that follows the content, which is what the virtualizer reads.

class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}

globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;

const VIEWPORT = { width: 800, height: 600 };

Object.defineProperty(HTMLElement.prototype, 'clientHeight', {
  configurable: true,
  get(this: HTMLElement) {
    return this.dataset.testid === 'feed-scroller' ? VIEWPORT.height : 0;
  }
});

Object.defineProperty(HTMLElement.prototype, 'clientWidth', {
  configurable: true,
  get(this: HTMLElement) {
    return this.dataset.testid === 'feed-scroller' ? VIEWPORT.width : 0;
  }
});

HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
  const height = this.dataset.testid === 'feed-scroller' ? VIEWPORT.height : 0;
  const width = this.dataset.testid === 'feed-scroller' ? VIEWPORT.width : 0;
  return { x: 0, y: 0, top: 0, left: 0, right: width, bottom: height, width, height, toJSON: () => ({}) };
};
