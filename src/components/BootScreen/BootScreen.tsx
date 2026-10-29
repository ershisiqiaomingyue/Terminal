import React, { useEffect } from "react";
import brandIcon from "@/assets/brand-icon.svg";
import "./boot-screen.css";

export const BootScreen: React.FC = () => {
  useEffect(() => {
    document.documentElement.classList.add("boot-mode");
    // Main-window Startup owns completion after its preload work settles.
  }, []);

  return (
    <main className="boot-screen" role="status" aria-live="polite" aria-label="Terminal 启动中">
      <span className="boot-orb" aria-hidden="true" />
      <section className="boot-content">
        <img className="boot-logo" src={brandIcon} width={84} height={84} alt="" aria-hidden="true" draggable={false} />
      </section>
    </main>
  );
};

export default BootScreen;
