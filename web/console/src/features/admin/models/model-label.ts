export function modelLabel(displayName: string, wireName: string) {
  return displayName === wireName ? wireName : `${displayName} (${wireName})`;
}
