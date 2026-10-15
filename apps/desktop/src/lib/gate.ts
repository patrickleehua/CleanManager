// Request generations: every view query takes a ticket; a response is applied only if its
// ticket is still the newest for that view. Older responses (a slow first page after the user
// already changed the sort, a layer for a directory they left) are dropped, never rendered.

export class RequestGate {
  private generation = 0;

  /** Start a new generation, invalidating every outstanding ticket. */
  next(): number {
    this.generation += 1;
    return this.generation;
  }

  /** The current ticket without invalidating (for follow-up pages of the same query). */
  current(): number {
    return this.generation;
  }

  isCurrent(ticket: number): boolean {
    return ticket === this.generation;
  }

  /**
   * Resolve to `{ stale: false, value }` only when the ticket is still current on arrival.
   * Errors of stale requests are swallowed the same way.
   */
  async run<T>(ticket: number, work: Promise<T>): Promise<{ stale: true } | { stale: false; value: T }> {
    let value: T;
    try {
      value = await work;
    } catch (e) {
      if (!this.isCurrent(ticket)) return { stale: true };
      throw e;
    }
    if (!this.isCurrent(ticket)) return { stale: true };
    return { stale: false, value };
  }
}
