import pygame
from pygame import Vector3

class Gem:
    def __init__(self, pos):
        self.pos = pos


class Game:
    def __init__(self):
        self.player = Vector3(0, 0, 0)
        self.xp = 0
        self.level = 1
        self.needed = 2
        self.gems = [Gem(Vector3(x, 0, 2)) for x in (3, 6, 9, 12, 15)]

    def update(self, dt, events):
        if pygame.key.get_pressed()[pygame.K_d]:
            self.player.x += 3.0 * dt
        for g in list(self.gems):
            to = self.player - g.pos
            d = to.length()
            if d < 4.0:
                g.pos += to * (min(8.0 * dt, d) / d)
                d = (self.player - g.pos).length()
            if d < 0.5:
                self.gems.remove(g)
                self.xp += 1
                if self.xp >= self.needed:
                    self.level += 1
                    self.xp = 0
                    self.needed += 2

    def draw(self, screen):
        screen.fill((0, 0, 0))
