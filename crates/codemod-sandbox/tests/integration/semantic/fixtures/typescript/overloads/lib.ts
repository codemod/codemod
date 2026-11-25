export function pick(value: string): string;
export function pick(value: number): number;
export function pick(value: string | number) {
  return value;
}

declare function ambient(): void;

export function useAmbient() {
  ambient();
}
