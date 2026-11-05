// 区画の番号。chizu-core の `Zone` の値と同じで、建物の静的データ（chizu-wasm の buildings）と
// チャンクの静的データのマスの区画の番号に使う。低密度の住宅は、高密度の住宅（1）とは別の番号 5 である。

export const ZONE_RESIDENTIAL = 1;
export const ZONE_COMMERCIAL = 2;
export const ZONE_INDUSTRIAL = 3;
export const ZONE_OFFICE = 4;
export const ZONE_RESIDENTIAL_LOW = 5;

/** 住宅か。低密度と高密度のどちらも住宅である。 */
export const isResidentialZone = (zone: number) =>
  zone === ZONE_RESIDENTIAL || zone === ZONE_RESIDENTIAL_LOW;
