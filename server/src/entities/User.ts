import { Entity, Column, PrimaryColumn, OneToMany, ManyToMany } from "typeorm";
import { File } from "./File";
import { Group } from "./Group";

@Entity()
export class User {
  @PrimaryColumn()
  username: string;

  @Column()
  password: string;

  @OneToMany(() => File, (file) => file.owner)
  files: File[];

  @ManyToMany(() => Group, (group) => group.users)
  groups: Group[];
}
