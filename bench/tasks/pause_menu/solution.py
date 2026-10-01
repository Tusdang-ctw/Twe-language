import pygame
from pygame import Vector3

class Game:
    def __init__(self):
        self.mode = "playing"
        self.elapsed = 0.0

    def update(self, dt, events):
        keys = {e.key for e in events if e.type == pygame.KEYDOWN}
        if self.mode == "playing":
            if pygame.K_ESCAPE in keys:
                self.mode = "paused"
            else:
                self.elapsed += dt
        elif self.mode == "paused":
            if pygame.K_ESCAPE in keys:
                self.mode = "playing"
            elif pygame.K_q in keys:
                self.mode = "confirm_quit"
        elif self.mode == "confirm_quit":
            if pygame.K_y in keys:
                self.mode = "title"
            elif pygame.K_n in keys:
                self.mode = "paused"
        elif self.mode == "title":
            if pygame.K_RETURN in keys:
                self.elapsed = 0.0
                self.mode = "playing"

    def draw(self, screen):
        screen.fill((0, 0, 0))
