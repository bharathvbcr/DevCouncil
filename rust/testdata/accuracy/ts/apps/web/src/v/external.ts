import { WidgetType } from '@codemirror/view';

// V20 (added for the dead-policy card, second pass): methods a library calls by
// name. CodeMirror calls `toDOM` on a widget it is handed; `StreamLanguage`
// calls `startState`/`token` on the parser object. No call in the corpus names
// any of them (DevPrism's ai-predictive-extension.ts and lang-bibtex.ts).
class GhostWidget extends WidgetType {
  toDOM(): string { return 'ghost'; }
}

const parser = {
  startState() { return {}; },
  token(_stream: unknown) { return null; },
};

export function makeEditor(): unknown[] {
  return [new GhostWidget(), parser];
}
