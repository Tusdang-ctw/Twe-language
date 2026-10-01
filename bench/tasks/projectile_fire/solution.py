import pygame
from pygame import Vector3

class Thing:
    def __init__(self, pos, hp=0):
        self.pos = pos
        self.hp = hp
        self.travelled = 0.0


class Game:
    def __init__(self):
        self.targets = [Thing(Vector3(6, 0, 0), hp=3)]
        self.bullets = []

    def update(self, dt, events):
        for e in events:
            if e.type == pygame.KEYDOWN and e.key == pygame.K_SPACE:
                self.bullets.append(Thing(Vector3(0, 0, 0)))
        for b in list(self.bullets):
            b.pos.x += 10.0 * dt
            b.travelled += 10.0 * dt
            hit = False
            for t in list(self.targets):
                if not hit and (t.pos - b.pos).length() < 0.5:
                    hit = True
                    t.hp -= 1
                    if t.hp <= 0:
                        self.targets.remove(t)
            if hit or b.travelled >= 8.0:
                self.bullets.remove(b)

    def draw(self, screen):
        screen.fill((0, 0, 0))
