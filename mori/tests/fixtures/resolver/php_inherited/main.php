<?php
interface Labeled { public function tag(): string; }
interface Named extends Labeled { public function label(): string; }
class Base { public function greet(): void {} }
class Middle extends Base {}
class Child extends Middle implements Named {
    public function label(): string { return ''; }
    public function tag(): string { return ''; }
}
class Other {
    public function greet(): void {}
    public function tag(): string { return ''; }
}
class InheritedProbe {
    public function run(Child $child, Named $named): void {
        $child->greet();
        $named->tag();
    }
}
