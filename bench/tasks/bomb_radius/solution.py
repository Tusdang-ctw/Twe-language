import pygame
from pygame import Vector3

class Enemy:
    def __init__(self, pos):
        self.pos = pos


class Game:
    def __init__(self):
        self.player = Vector3(0, 0, 0)
        self.score = 0
        self.enemies = [Enemy(Vector3(*p)) for p in ((2, 0, 0), (4, 0, 0), (4, 0, 3), (9, 0, 0), (12, 0, 1))]

    def update(self, dt, events):
        if pygame.key.get_pressed()[pygame.K_d]:
            self.player.x += 5.0 * dt
        if any(e.type == pygame.KEYDOWN and e.key == pygame.K_b for e in events):
            for e in list(self.enemies):
                if (e.pos - self.player).length() <= 3.0:
                    self.enemies.remove(e)
                    self.score += 10

    def draw(self, screen):
        screen.fill((0, 0, 0))
