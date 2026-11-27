static int alpha_target(void){return 1;}
static int bravo_target(void){return 2;}
int occurrence_probe(void){
 int (*fp)(void) = alpha_target;
 /*xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx*/ fp ( );
#define HIDDEN_FP_CALL() (fp = bravo_target, fp())
 return HIDDEN_FP_CALL();
}
