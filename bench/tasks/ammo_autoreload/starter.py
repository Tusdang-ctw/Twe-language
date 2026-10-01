import pygame
from pygame import Vector3

class Game:
    def __init__(self):
        self.ammo = 6
        self.reserve = 18
        self.shots = 0
        self.reloading = False
        self.reload_left = 0.0

    def update(self, dt, events):
        keys = {e.key for e in events if e.type == pygame.KEYDOWN}
        if self.reloading:
            self.reload_left -= dt
            if self.reload_left <= 0.0:
                moved = min(6 - self.ammo, self.reserve)
                self.ammo += moved
                self.reserve -= moved
                self.reloading = False
        elif pygame.K_r in keys and self.ammo < 6 and self.reserve > 0:
            self.reloading = True
            self.reload_left = 1.5
        elif pygame.K_SPACE in keys and self.ammo > 0:
            self.ammo -= 1
            self.shots += 1

    def draw(self, screen):
        screen.fill((0, 0, 0))
