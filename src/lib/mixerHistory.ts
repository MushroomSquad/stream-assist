export function createHistory<T>(initial: T, limit = 100) {
  const values = [structuredClone(initial)];
  let index = 0;
  return {
    push(v: T) {
      values.splice(index + 1);
      values.push(structuredClone(v));
      if (values.length > Math.max(1, limit)) values.shift();
      index = values.length - 1;
    },
    undo(): T { index = Math.max(0, index - 1); return structuredClone(values[index]); },
    redo(): T { index = Math.min(values.length - 1, index + 1); return structuredClone(values[index]); },
    get canUndo() { return index > 0; },
    get canRedo() { return index < values.length - 1; },
  };
}
