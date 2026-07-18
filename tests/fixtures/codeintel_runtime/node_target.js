function child(value) {
  return value + 1;
}

function parent() {
  let result = 0;
  const start = Date.now();
  // Loop for ~300ms of wall time so the 1ms sampling profiler reliably
  // captures parent/child frames even on fast machines.
  while (Date.now() - start < 300) {
    for (let index = 0; index < 1000000; index += 1) {
      result = child(result);
    }
  }
  return result;
}

console.log(parent() > 0 ? 42 : 0);
