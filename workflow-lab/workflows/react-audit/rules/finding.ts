export interface Finding {
  rule: string;
  line: number;
  dims: Record<string, string>;
}
