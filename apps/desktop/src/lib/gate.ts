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

  /** Start a new generation for `work` and resolve it only if nothing newer started meanwhile. */
  latest<T>(work: Promise<T>): Promise<{ stale: true } | { stale: false; value: T }> {
    return this.run(this.next(), work);
  }

  /** Invalidate every outstanding ticket (the view's subject changed or the view closed). */
  invalidate(): void {
    this.next();
  }
}

/**
 * GUI-14: an asynchronous result is applied to a view only while it is the newest request *for
 * the subject the view shows now* (the selected app, the open tab). A result for an older subject
 * or an older request of the same subject is dropped — it never replaces the current view.
 */
export class SubjectGate<K> {
  private gate = new RequestGate();
  private subject: K | null = null;

  /** The subject the view shows now. Changing it invalidates every pending result. */
  show(subject: K): void {
    if (subject !== this.subject) {
      this.subject = subject;
      this.gate.invalidate();
    }
  }

  get current(): K | null {
    return this.subject;
  }

  /** Load data for `subject`; the value is returned only if the view still shows it. */
  async load<T>(subject: K, work: Promise<T>): Promise<{ stale: true } | { stale: false; value: T }> {
    this.show(subject);
    const r = await this.gate.latest(work);
    if (r.stale || subject !== this.subject) return { stale: true };
    return r;
  }
}

