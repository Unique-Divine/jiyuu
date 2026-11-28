<?php
class Alpha { public function render(): void {} }
class Beta { public function render(): void {} }
class OccurrenceProbe {
    public function run(Alpha $alpha, Beta $beta): void {
        $alpha->render();
        $beta->render();
    }
}
