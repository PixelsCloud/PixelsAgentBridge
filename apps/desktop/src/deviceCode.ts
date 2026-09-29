export function formatDeviceCode(code: string): string {
  if (!/^\d{1,9}$/.test(code)) return code;
  return code.replace(/(\d{3})(?=\d)/g, "$1 ");
}

export function normalizeDeviceCode(value: string): string {
  return value.replace(/\D/g, "").slice(0, 9);
}
