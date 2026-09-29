import { Monitor } from "lucide-react";
import windowsLogo from "./assets/os/windows.svg";
import macosLogo from "./assets/os/macos.svg";
import linuxLogo from "./assets/os/linux.svg";

export function OsLogo({ family }: { family: string }) {
  const os = family.toLowerCase();
  let logo: string | undefined;

  if (os.includes("windows")) {
    logo = windowsLogo;
  } else if (os.includes("macos") || os.includes("darwin")) {
    logo = macosLogo;
  } else if (os.includes("linux")) {
    logo = linuxLogo;
  }

  if (logo) return <img src={logo} alt="" aria-hidden="true" />;
  return <Monitor size={18} aria-hidden="true" />;
}
