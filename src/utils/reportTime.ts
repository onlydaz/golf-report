import dayjs, { type Dayjs } from "dayjs";
import utc from "dayjs/plugin/utc";
import timezone from "dayjs/plugin/timezone";

dayjs.extend(utc);
dayjs.extend(timezone);
export const REPORT_TIME_ZONE = "Asia/Ho_Chi_Minh";
export const vietnamNow = () => dayjs().tz(REPORT_TIME_ZONE);

// Calendar dates selected by QC belong to Vietnam, independently of Windows timezone.
export const reportDateBoundary = (date: Dayjs, end = false): string =>
  dayjs(`${date.add(end ? 1 : 0, "day").format("YYYY-MM-DD")}T00:00:00.000+07:00`).toISOString();

export function formatReportTime(value: string): string {
  const instant = dayjs(value);
  return instant.isValid()
    ? instant.tz(REPORT_TIME_ZONE).format("DD/MM/YYYY HH:mm:ss")
    : "—";
}
