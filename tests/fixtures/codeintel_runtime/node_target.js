function child(value) {
  return value + 1;
}

function parent() {
  let result = 0;
  for (let index = 0; index < 5000000; index += 1) {
    result = child(result);
  }
  return result;
}

console.log(parent() > 0 ? 42 : 0);
