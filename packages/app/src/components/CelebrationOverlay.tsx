import { useEffect, useState, useCallback } from "react";
import { cn } from "@/lib/utils";
import { useOnboardingStore } from "@/stores/onboarding-store";

interface Particle {
  id: number;
  x: number;
  y: number;
  color: string;
  size: number;
  velocityX: number;
  velocityY: number;
  rotation: number;
  rotationSpeed: number;
}

const COLORS = ["#3b82f6", "#10b981", "#f59e0b", "#ef4444", "#8b5cf6", "#ec4899"];

function createParticle(id: number): Particle {
  return {
    id,
    x: 50 + (Math.random() - 0.5) * 20,
    y: 50,
    color: COLORS[Math.floor(Math.random() * COLORS.length)]!,
    size: 4 + Math.random() * 4,
    velocityX: (Math.random() - 0.5) * 8,
    velocityY: -8 - Math.random() * 6,
    rotation: Math.random() * 360,
    rotationSpeed: (Math.random() - 0.5) * 20,
  };
}

export function CelebrationOverlay() {
  const guidedFlowStep = useOnboardingStore((s) => s.guidedFlowStep);
  const [particles, setParticles] = useState<Particle[]>([]);
  const [visible, setVisible] = useState(false);

  const triggerCelebration = useCallback(() => {
    // Create initial burst of particles
    const newParticles = Array.from({ length: 30 }, (_, i) => createParticle(i));
    setParticles(newParticles);
    setVisible(true);

    // Animate particles
    let frame = 0;
    const maxFrames = 60;
    const animate = () => {
      frame++;
      if (frame > maxFrames) {
        setVisible(false);
        setParticles([]);
        return;
      }

      setParticles((prev) =>
        prev.map((p) => ({
          ...p,
          x: p.x + p.velocityX * 0.3,
          y: p.y + p.velocityY * 0.3,
          velocityY: p.velocityY + 0.4, // gravity
          rotation: p.rotation + p.rotationSpeed,
        }))
      );

      requestAnimationFrame(animate);
    };

    requestAnimationFrame(animate);
  }, []);

  // Trigger when reaching celebrate step
  useEffect(() => {
    if (guidedFlowStep === "celebrate") {
      triggerCelebration();
    }
  }, [guidedFlowStep, triggerCelebration]);

  // Trigger on sign-in / upgrade celebration events
  useEffect(() => {
    const handle = () => triggerCelebration();
    window.addEventListener("vcad:celebrate-sign-in", handle);
    window.addEventListener("vcad:celebrate-upgrade", handle);
    return () => {
      window.removeEventListener("vcad:celebrate-sign-in", handle);
      window.removeEventListener("vcad:celebrate-upgrade", handle);
    };
  }, [triggerCelebration]);

  if (!visible) return null;

  return (
    <div className="fixed inset-0 z-50 pointer-events-none overflow-hidden">
      {particles.map((p) => (
        <div
          key={p.id}
          className={cn(
            "absolute rounded-sm",
            "transition-opacity duration-300"
          )}
          style={{
            left: `${p.x}%`,
            top: `${p.y}%`,
            width: p.size,
            height: p.size,
            backgroundColor: p.color,
            transform: `rotate(${p.rotation}deg)`,
            opacity: Math.max(0, 1 - p.y / 150),
          }}
        />
      ))}
    </div>
  );
}
