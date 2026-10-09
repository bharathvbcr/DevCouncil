export class Gateway {
  buildUrl(p: string): string { return p; }
  open(): void {
    const connect = async (p: string) => { this.buildUrl(p); };
    void connect('a');
  }
}
